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

# The value is set outside OpenTofu so write credentials never enter state.
resource "aws_secretsmanager_secret" "cli_database_url" {
  count                   = local.cli_enabled ? 1 : 0
  name                    = "${var.name}/cli-database-url"
  description             = "TLS PostgreSQL URL for the Ultrafinance CLI import role"
  recovery_window_in_days = 7
}

resource "aws_s3_bucket" "cli_inputs" {
  count         = local.cli_enabled ? 1 : 0
  bucket_prefix = "${var.name}-cli-inputs-"
}

resource "aws_s3_bucket_public_access_block" "cli_inputs" {
  count                   = local.cli_enabled ? 1 : 0
  bucket                  = aws_s3_bucket.cli_inputs[0].id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "cli_inputs" {
  count  = local.cli_enabled ? 1 : 0
  bucket = aws_s3_bucket.cli_inputs[0].id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_policy" "cli_inputs" {
  count  = local.cli_enabled ? 1 : 0
  bucket = aws_s3_bucket.cli_inputs[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Deny", Principal = "*", Action = "s3:*"
      Resource  = [aws_s3_bucket.cli_inputs[0].arn, "${aws_s3_bucket.cli_inputs[0].arn}/*"]
      Condition = { Bool = { "aws:SecureTransport" = "false" } }
    }]
  })
}

resource "aws_s3_bucket_lifecycle_configuration" "cli_inputs" {
  count  = local.cli_enabled ? 1 : 0
  bucket = aws_s3_bucket.cli_inputs[0].id
  rule {
    id     = "expire-staged-inputs"
    status = "Enabled"
    filter { prefix = "jobs/" }
    expiration { days = 7 }
    abort_incomplete_multipart_upload { days_after_initiation = 1 }
  }
}

resource "aws_iam_role" "cli_execution" {
  count              = local.cli_enabled ? 1 : 0
  name               = "${var.name}-cli-execution"
  assume_role_policy = local.cli_trust
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
      { Effect = "Allow", Action = "secretsmanager:GetSecretValue", Resource = aws_secretsmanager_secret.cli_database_url[0].arn }
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
      Effect = "Allow", Action = "s3:GetObject", Resource = "${aws_s3_bucket.cli_inputs[0].arn}/jobs/*"
    }]
  })
}

resource "aws_ecs_task_definition" "cli" {
  count                    = local.cli_enabled ? 1 : 0
  family                   = "${var.name}-cli"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = "512"
  memory                   = "1024"
  execution_role_arn       = aws_iam_role.cli_execution[0].arn
  task_role_arn            = aws_iam_role.cli_task[0].arn
  runtime_platform {
    cpu_architecture        = "ARM64"
    operating_system_family = "LINUX"
  }
  volume { name = "input" }
  container_definitions = jsonencode([
    {
      name       = "input", image = var.cli_download_image, essential = false
      entryPoint = ["/bin/sh", "-c"]
      command    = ["if [ -n \"$INPUT_KEY\" ]; then exec aws s3 cp \"s3://$INPUT_BUCKET/$INPUT_KEY\" /input/source --only-show-errors; fi"]
      environment = [
        { name = "INPUT_BUCKET", value = aws_s3_bucket.cli_inputs[0].id },
        { name = "INPUT_KEY", value = "" },
        { name = "AWS_DEFAULT_REGION", value = var.aws_region }
      ]
      mountPoints = [{ sourceVolume = "input", containerPath = "/input", readOnly = false }]
      logConfiguration = {
        logDriver = "awslogs"
        options   = { awslogs-group = aws_cloudwatch_log_group.cli[0].name, awslogs-region = var.aws_region, awslogs-stream-prefix = "cli" }
      }
    },
    {
      name         = "cli", image = var.image_uri, essential = true
      entryPoint   = ["/usr/local/bin/ultrafinance"]
      command      = ["merchants", "list", "--json"]
      dependsOn    = [{ containerName = "input", condition = "SUCCESS" }]
      startTimeout = 120
      secrets      = [{ name = "ULTRAFINANCE_DATABASE_URL", valueFrom = aws_secretsmanager_secret.cli_database_url[0].arn }]
      mountPoints  = [{ sourceVolume = "input", containerPath = "/input", readOnly = true }]
      logConfiguration = {
        logDriver = "awslogs"
        options   = { awslogs-group = aws_cloudwatch_log_group.cli[0].name, awslogs-region = var.aws_region, awslogs-stream-prefix = "cli" }
      }
    }
  ])
  lifecycle {
    precondition {
      condition     = var.enable_aurora && var.image_uri != null
      error_message = "CLI tasks require enable_aurora=true and an immutable Ultrafinance image_uri."
    }
  }
}
