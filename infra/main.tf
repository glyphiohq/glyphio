# SPDX-License-Identifier: Apache-2.0
# Glyphio sync server — AWS serverless reference deployment.
#
# ECR (container image) → Lambda (image + AWS Lambda Web Adapter baked into the image's
# `lambda` target) → API Gateway HTTP API (TLS + throttling) → DynamoDB (single table + GSI).
# Everything is parameterized in variables.tf; there are no organization-specific defaults.

terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 5.0"
    }
  }
}

provider "aws" {
  region = var.region
}

# ---- container registry ------------------------------------------------------

resource "aws_ecr_repository" "server" {
  name                 = "${var.name_prefix}-server"
  image_tag_mutability = "MUTABLE"

  image_scanning_configuration {
    scan_on_push = true
  }
}

resource "aws_ecr_lifecycle_policy" "keep_recent" {
  repository = aws_ecr_repository.server.name
  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "keep the 10 most recent images"
      selection = {
        tagStatus   = "any"
        countType   = "imageCountMoreThan"
        countNumber = 10
      }
      action = { type = "expire" }
    }]
  })
}

# ---- data --------------------------------------------------------------------

resource "aws_dynamodb_table" "records" {
  name         = "${var.name_prefix}-records"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "pk"
  range_key    = "sk"

  attribute {
    name = "pk"
    type = "S"
  }
  attribute {
    name = "sk"
    type = "S"
  }
  attribute {
    name = "team"
    type = "S"
  }
  attribute {
    name = "seq"
    type = "N"
  }

  global_secondary_index {
    name            = "by-seq"
    hash_key        = "team"
    range_key       = "seq"
    projection_type = "ALL"
  }

  point_in_time_recovery {
    enabled = true
  }
}

# ---- function ----------------------------------------------------------------

data "aws_iam_policy_document" "assume" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "lambda" {
  name               = "${var.name_prefix}-server"
  assume_role_policy = data.aws_iam_policy_document.assume.json
}

# Least privilege: exactly the four DynamoDB actions the server issues, on its table + GSI only.
data "aws_iam_policy_document" "dynamo" {
  statement {
    actions = [
      "dynamodb:GetItem",
      "dynamodb:PutItem",
      "dynamodb:UpdateItem",
      "dynamodb:Query",
    ]
    resources = [
      aws_dynamodb_table.records.arn,
      "${aws_dynamodb_table.records.arn}/index/by-seq",
    ]
  }
}

resource "aws_iam_role_policy" "dynamo" {
  name   = "dynamo-access"
  role   = aws_iam_role.lambda.id
  policy = data.aws_iam_policy_document.dynamo.json
}

resource "aws_iam_role_policy_attachment" "logs" {
  role       = aws_iam_role.lambda.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

resource "aws_cloudwatch_log_group" "lambda" {
  name              = "/aws/lambda/${var.name_prefix}-server"
  retention_in_days = var.log_retention_days
}

resource "aws_lambda_function" "server" {
  function_name = "${var.name_prefix}-server"
  role          = aws_iam_role.lambda.arn
  package_type  = "Image"
  image_uri     = "${aws_ecr_repository.server.repository_url}:${var.image_tag}"
  timeout       = 30
  memory_size   = var.lambda_memory_mb
  architectures = [var.lambda_architecture]

  environment {
    variables = {
      STORAGE            = "dynamo"
      DYNAMO_TABLE       = aws_dynamodb_table.records.name
      OIDC_ISSUER        = var.oidc_issuer
      OIDC_AUDIENCE      = var.oidc_audience
      TEAMS_CLAIM        = var.teams_claim
      RATE_LIMIT_PER_MIN = tostring(var.rate_limit_per_min)
      RUST_LOG           = var.rust_log
    }
  }

  depends_on = [aws_cloudwatch_log_group.lambda]
}

# ---- HTTP API (TLS + edge throttling) -----------------------------------------

resource "aws_apigatewayv2_api" "http" {
  name          = "${var.name_prefix}-api"
  protocol_type = "HTTP"
}

resource "aws_apigatewayv2_integration" "lambda" {
  api_id                 = aws_apigatewayv2_api.http.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.server.invoke_arn
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "default" {
  api_id    = aws_apigatewayv2_api.http.id
  route_key = "$default"
  target    = "integrations/${aws_apigatewayv2_integration.lambda.id}"
}

resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.http.id
  name        = "$default"
  auto_deploy = true

  default_route_settings {
    throttling_burst_limit = var.throttle_burst
    throttling_rate_limit  = var.throttle_rate
  }
}

resource "aws_lambda_permission" "apigw" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.server.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.http.execution_arn}/*/*"
}
