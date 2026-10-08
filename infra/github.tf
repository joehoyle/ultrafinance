locals {
  enable_ci = var.github_repository != null
  # Name-based subjects work for older repos. New repos include immutable IDs.
  github_subjects = !local.enable_ci ? [] : concat(
    ["repo:${var.github_repository}:ref:refs/heads/main"],
    var.github_owner_id != null && var.github_repository_id != null ? [
      "repo:${split("/", var.github_repository)[0]}@${var.github_owner_id}/${split("/", var.github_repository)[1]}@${var.github_repository_id}:ref:refs/heads/main"
    ] : []
  )
}

resource "aws_iam_openid_connect_provider" "github" {
  count          = local.enable_ci && var.github_oidc_provider_arn == null ? 1 : 0
  url            = "https://token.actions.githubusercontent.com"
  client_id_list = ["sts.amazonaws.com"]
}

resource "aws_iam_role" "deploy" {
  count = local.enable_ci ? 1 : 0
  name  = "${var.name}-github-deploy"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Federated = var.github_oidc_provider_arn != null ? var.github_oidc_provider_arn : aws_iam_openid_connect_provider.github[0].arn
      }
      Action = "sts:AssumeRoleWithWebIdentity"
      Condition = {
        StringEquals = {
          "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
          "token.actions.githubusercontent.com:sub" = local.github_subjects
        }
      }
    }]
  })
  lifecycle {
    precondition {
      condition     = (var.github_owner_id == null) == (var.github_repository_id == null)
      error_message = "Set both GitHub numeric IDs, or leave both null."
    }
  }
}

# CI cannot change IAM, function URLs, environment secrets or infrastructure.
resource "aws_iam_role_policy" "deploy" {
  count = local.enable_ci ? 1 : 0
  role  = aws_iam_role.deploy[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = concat([
      {
        Effect = "Allow", Action = "ecr:GetAuthorizationToken", Resource = "*"
      },
      {
        Effect   = "Allow"
        Action   = ["ecr:BatchCheckLayerAvailability", "ecr:InitiateLayerUpload", "ecr:UploadLayerPart", "ecr:CompleteLayerUpload", "ecr:PutImage", "ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer", "ecr:DescribeImages"]
        Resource = aws_ecr_repository.app.arn
      }
      ], local.deploy_app ? [
      {
        Effect   = "Allow"
        Action   = ["lambda:GetFunction", "lambda:GetFunctionConfiguration", "lambda:ListVersionsByFunction", "lambda:UpdateFunctionCode", "lambda:PublishVersion"]
        Resource = aws_lambda_function.app[0].arn
      },
      {
        Effect = "Allow", Action = ["lambda:GetAlias", "lambda:UpdateAlias"]
        # Alias management APIs authorize against the unqualified function ARN.
        Resource = aws_lambda_function.app[0].arn
      },
      {
        Effect   = "Allow", Action = ["lambda:InvokeFunction"]
        Resource = "${aws_lambda_function.app[0].arn}:*"
      }
    ] : [])
  })
}
