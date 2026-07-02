# SPDX-License-Identifier: Apache-2.0

output "api_endpoint" {
  description = "Backend base URL — paste into the Glyphio app's sync settings."
  value       = aws_apigatewayv2_api.http.api_endpoint
}

output "ecr_repository_url" {
  description = "Push the server image (Dockerfile target `lambda`) here."
  value       = aws_ecr_repository.server.repository_url
}

output "dynamodb_table" {
  value = aws_dynamodb_table.records.name
}
