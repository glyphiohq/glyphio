# Glyphio sync server — AWS reference deployment

Serverless: **ECR → Lambda (container image + web adapter) → API Gateway HTTP API →
DynamoDB**. Near-zero idle cost, nothing to patch, TLS and edge throttling from API Gateway,
least-privilege IAM (the function can only touch its own table). All parameters are variables —
no organization-specific values live in this repo.

## Deploy

```bash
# 0. prerequisites: terraform >= 1.5, docker, AWS credentials for the target account

cd infra
cp terraform.tfvars.example terraform.tfvars   # fill in region + OIDC values

# 1. create the ECR repo (and everything else will fail on the missing image — that's fine,
#    or create just the repo first):
terraform init
terraform apply -target=aws_ecr_repository.server

# 2. build + push the Lambda image (from the REPOSITORY ROOT; arm64 matches the default
#    lambda_architecture — pass --platform linux/amd64 and set lambda_architecture=x86_64 otherwise):
REPO=$(terraform output -raw ecr_repository_url)
aws ecr get-login-password --region <AWS_REGION> | docker login --username AWS --password-stdin "${REPO%%/*}"
docker build -f ../server/Dockerfile --target lambda --platform linux/arm64 -t "$REPO:latest" ..
docker push "$REPO:latest"

# 3. everything else:
terraform apply

# 4. the app-facing URL:
terraform output api_endpoint
```

Paste `api_endpoint` into the Glyphio app's Sync settings (see the repo's `SETUP.md`). Redeploy
a new image by pushing a new tag and `terraform apply -var image_tag=<tag>` (or re-push `latest`
and update the function).

## Static-token mode

The reference deployment assumes OIDC. To also allow static tokens (e.g. service accounts),
add a `STATIC_TOKENS` entry to the Lambda `environment` block — hashes only, and prefer
wiring it through AWS Secrets Manager rather than tfvars if the token list is sensitive.

## Notes

- DynamoDB is on-demand billing with point-in-time recovery enabled.
- API Gateway throttling (`throttle_burst`/`throttle_rate`) is the outer rate limit; the
  server enforces a per-credential limit (`rate_limit_per_min`) inside.
- CloudWatch logs retain for `log_retention_days` (default 30); the server never logs tokens
  or record bodies.
