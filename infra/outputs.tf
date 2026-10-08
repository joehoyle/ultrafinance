output "repository_url" {
  value = aws_ecr_repository.app.repository_url
}
output "site_url" {
  value = local.deploy_app ? "https://${var.domain_name == null ? aws_cloudfront_distribution.app[0].domain_name : var.domain_name}" : null
}
output "function_url" {
  description = "Public direct Lambda endpoint."
  value       = local.deploy_app ? aws_lambda_function_url.app[0].function_url : null
}

output "function_name" {
  value = local.deploy_app ? aws_lambda_function.app[0].function_name : null
}
output "aws_region" {
  value = var.aws_region
}
output "aws_profile" {
  value = var.aws_profile
}
output "deploy_role_arn" {
  value = local.enable_ci ? aws_iam_role.deploy[0].arn : null
}
