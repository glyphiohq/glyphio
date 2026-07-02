# SPDX-License-Identifier: Apache-2.0
# All deployment parameters. No organization-specific defaults — supply real values via
# terraform.tfvars (see terraform.tfvars.example) or -var flags.

variable "region" {
  description = "AWS region to deploy into."
  type        = string
}

variable "name_prefix" {
  description = "Prefix for all resource names (e.g. glyphio-sandbox)."
  type        = string
  default     = "glyphio"
}

variable "image_tag" {
  description = "Tag of the server image (Dockerfile target `lambda`) pushed to the ECR repo."
  type        = string
  default     = "latest"
}

variable "oidc_issuer" {
  description = "OIDC issuer URL whose JWKS validates bearer tokens (the same issuer the app signs in against)."
  type        = string
}

variable "oidc_audience" {
  description = "Expected `aud` claim — the Glyphio app's OIDC client ID."
  type        = string
}

variable "teams_claim" {
  description = "JWT claim carrying the user's team names (array of strings)."
  type        = string
  default     = "groups"
}

variable "rate_limit_per_min" {
  description = "Per-credential request budget per minute enforced inside the server."
  type        = number
  default     = 60
}

variable "throttle_burst" {
  description = "API Gateway burst limit (requests)."
  type        = number
  default     = 50
}

variable "throttle_rate" {
  description = "API Gateway steady-state rate limit (requests/second)."
  type        = number
  default     = 25
}

variable "lambda_memory_mb" {
  description = "Lambda memory (MB)."
  type        = number
  default     = 256
}

variable "lambda_architecture" {
  description = "Lambda CPU architecture (arm64 is cheaper; must match the pushed image)."
  type        = string
  default     = "arm64"
}

variable "log_retention_days" {
  description = "CloudWatch log retention."
  type        = number
  default     = 30
}

variable "rust_log" {
  description = "tracing filter for the server."
  type        = string
  default     = "info"
}
