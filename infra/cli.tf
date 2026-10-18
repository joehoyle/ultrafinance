locals {
  cli_enabled = var.enable_aurora && local.deploy_app
  cli_trust = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow", Principal = { Service = "ecs-tasks.amazonaws.com" }, Action = "sts:AssumeRole"
    }]
  })
}

resource "aws_ecs_cluster" "cli" {
  count = local.cli_enabled ? 1 : 0
  name  = "${var.name}-cli"
}

resource "aws_cloudwatch_log_group" "cli" {
  count             = local.cli_enabled ? 1 : 0
  name              = "/ecs/${var.name}-cli"
  retention_in_days = 14
}

# Preserve the existing application URL secret; administrator shells use the RDS-managed secret.
resource "aws_secretsmanager_secret" "cli_database_url" {
  count                   = local.cli_enabled ? 1 : 0
  name                    = "${var.name}/cli-database-url"
  description             = "TLS PostgreSQL application URL"
  recovery_window_in_days = 7
}

resource "aws_secretsmanager_secret_version" "cli_database_url" {
  count         = local.cli_enabled && nonsensitive(var.database_url != null) ? 1 : 0
  secret_id     = aws_secretsmanager_secret.cli_database_url[0].id
  secret_string = var.database_url
}

resource "aws_iam_role" "cli_execution" {
  count              = local.cli_enabled ? 1 : 0
  name               = "${var.name}-cli-execution"
  assume_role_policy = local.cli_trust
}

resource "aws_secretsmanager_secret" "cli_api_keys" {
  count = local.cli_enabled ? 1 : 0
  name  = "${var.name}/cli-api-keys"
}

resource "aws_secretsmanager_secret_version" "cli_api_keys" {
  count     = local.cli_enabled ? 1 : 0
  secret_id = aws_secretsmanager_secret.cli_api_keys[0].id
  secret_string = jsonencode({
    TYPESAFE_API_KEY              = var.typesafe_api_key
    OPENAI_API_KEY                = var.openai_api_key
    ULTRAFINANCE_FOURSQUARE_TOKEN = var.foursquare_token
  })
}

resource "aws_iam_role_policy" "cli_execution" {
  count = local.cli_enabled ? 1 : 0
  role  = aws_iam_role.cli_execution[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      { Effect = "Allow", Action = "ecr:GetAuthorizationToken", Resource = "*" },
      { Effect = "Allow", Action = ["ecr:BatchCheckLayerAvailability", "ecr:GetDownloadUrlForLayer", "ecr:BatchGetImage"], Resource = aws_ecr_repository.app.arn },
      { Effect = "Allow", Action = ["logs:CreateLogStream", "logs:PutLogEvents"], Resource = "${aws_cloudwatch_log_group.cli[0].arn}:*" },
      { Effect = "Allow", Action = "secretsmanager:GetSecretValue", Resource = aws_rds_cluster.database[0].master_user_secret[0].secret_arn },
      { Effect = "Allow", Action = "secretsmanager:GetSecretValue", Resource = aws_secretsmanager_secret.cli_api_keys[0].arn }
    ]
  })
}

resource "aws_iam_role" "cli_task" {
  count              = local.cli_enabled ? 1 : 0
  name               = "${var.name}-cli-task"
  assume_role_policy = local.cli_trust
}

resource "aws_iam_role_policy" "cli_task" {
  count = local.cli_enabled ? 1 : 0
  role  = aws_iam_role.cli_task[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow", Action = ["ssmmessages:CreateControlChannel", "ssmmessages:CreateDataChannel", "ssmmessages:OpenControlChannel", "ssmmessages:OpenDataChannel"], Resource = "*"
    }]
  })
}

resource "aws_ecs_task_definition" "cli" {
  count                    = local.cli_enabled ? 1 : 0
  family                   = "${var.name}-cli"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = "8192"
  memory                   = "32768"
  execution_role_arn       = aws_iam_role.cli_execution[0].arn
  task_role_arn            = aws_iam_role.cli_task[0].arn
  volume {
    name                = "scratch"
    configure_at_launch = true
  }
  runtime_platform {
    cpu_architecture        = "ARM64"
    operating_system_family = "LINUX"
  }
  container_definitions = jsonencode([{
    name                   = "cli", image = var.image_uri, essential = true
    entryPoint             = ["/bin/sh", "-c"]
    command                = ["mkdir -p /scratch/tmp && exec sleep ${var.cli_lifetime_hours * 3600}"]
    workingDirectory       = "/scratch"
    mountPoints            = [{ sourceVolume = "scratch", containerPath = "/scratch", readOnly = false }]
    linuxParameters        = { initProcessEnabled = true }
    readonlyRootFilesystem = false
    environment = [
      { name = "TMPDIR", value = "/scratch/tmp" },
      { name = "ULTRAFINANCE_RESEARCH_MODEL", value = var.research_model },
      { name = "ULTRAFINANCE_LOGO_BUCKET", value = aws_s3_bucket.logos[0].id },
      { name = "ULTRAFINANCE_LOGO_BASE_URL", value = "https://${var.domain_name == null ? aws_cloudfront_distribution.app[0].domain_name : var.domain_name}" },
      { name = "ULTRAFINANCE_DATABASE_HOST", value = aws_rds_cluster.database[0].endpoint },
      { name = "ULTRAFINANCE_DATABASE_NAME", value = aws_rds_cluster.database[0].database_name }
    ]
    secrets = [
      { name = "ULTRAFINANCE_ADMIN_USERNAME", valueFrom = "${aws_rds_cluster.database[0].master_user_secret[0].secret_arn}:username::" },
      { name = "ULTRAFINANCE_ADMIN_PASSWORD", valueFrom = "${aws_rds_cluster.database[0].master_user_secret[0].secret_arn}:password::" },
      { name = "OPENAI_API_KEY", valueFrom = "${aws_secretsmanager_secret.cli_api_keys[0].arn}:OPENAI_API_KEY::" },
      { name = "TYPESAFE_API_KEY", valueFrom = "${aws_secretsmanager_secret.cli_api_keys[0].arn}:TYPESAFE_API_KEY::" },
      { name = "ULTRAFINANCE_FOURSQUARE_TOKEN", valueFrom = "${aws_secretsmanager_secret.cli_api_keys[0].arn}:ULTRAFINANCE_FOURSQUARE_TOKEN::" }
    ]
    logConfiguration = {
      logDriver = "awslogs"
      options   = { awslogs-group = aws_cloudwatch_log_group.cli[0].name, awslogs-region = var.aws_region, awslogs-stream-prefix = "cli" }
    }
  }])
  depends_on = [aws_iam_role_policy.cli_execution, aws_secretsmanager_secret_version.cli_api_keys]
  lifecycle {
    precondition {
      condition     = var.enable_aurora && var.image_uri != null
      error_message = "CLI tasks require enable_aurora=true and an immutable Ultrafinance image_uri."
    }
  }
}

# ECS provisions and deletes a fresh encrypted scratch volume for each session.
resource "aws_iam_role" "cli_volumes" {
  count = local.cli_enabled ? 1 : 0
  name  = "${var.name}-cli-volumes"
  assume_role_policy = jsonencode({
    Version   = "2012-10-17"
    Statement = [{ Effect = "Allow", Principal = { Service = "ecs.amazonaws.com" }, Action = "sts:AssumeRole" }]
  })
}

resource "aws_iam_role_policy_attachment" "cli_volumes" {
  count      = local.cli_enabled ? 1 : 0
  role       = aws_iam_role.cli_volumes[0].name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSInfrastructureRolePolicyForVolumes"
}
