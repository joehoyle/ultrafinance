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

output "database_endpoint" {
  description = "Private Aurora writer endpoint; initialize schema/data from inside the VPC."
  value       = var.enable_aurora ? aws_rds_cluster.database[0].endpoint : null
}
output "database_admin_secret_arn" {
  description = "RDS-managed administrator secret. Do not use the administrator as Lambda's runtime role."
  value       = var.enable_aurora ? aws_rds_cluster.database[0].master_user_secret[0].secret_arn : null
}
output "database_client_security_group_id" {
  value = var.enable_aurora ? aws_security_group.database_client[0].id : null
}
output "database_private_subnet_ids" {
  value = aws_subnet.database[*].id
}

output "cli_runner" {
  description = "Nonsecret configuration for the infra CLI subcommands."
  value = local.cli_enabled ? {
    cluster         = aws_ecs_cluster.cli[0].arn
    task_definition = aws_ecs_task_definition.cli[0].arn
    database_secret = aws_rds_cluster.database[0].master_user_secret[0].secret_arn
    subnets         = aws_subnet.database[*].id
    security_groups = [aws_security_group.database_client[0].id]
    log_group       = aws_cloudwatch_log_group.cli[0].name
    function_name   = aws_lambda_function.app[0].function_name
  } : null
}
