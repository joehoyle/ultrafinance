terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 4.0"
    }
  }
}

variable "ecr_image_revision" {
  type = string
  default = "latest"
}

variable "cookie_secret" {
  type = string
}

variable "nordigen_secret_id" {
  type = string
}

variable "nordigen_secret_key" {
  type = string
}

variable "ntropy_api_key" {
  type = string
}

provider "aws" {
  region  = "eu-central-1"
  profile = "sub_account"
}

provider "aws" {
  region  = "us-east-1"
  alias = "us-east-1"
  profile = "sub_account"
}

resource "aws_vpc" "default" {
  cidr_block = "10.0.0.0/16"
    tags = {
    Name = "Ultrafinance"
  }
  enable_dns_support = true
  enable_dns_hostnames = true
}

resource "aws_subnet" "a" {
  vpc_id            = aws_vpc.default.id
  cidr_block        = "10.0.1.0/24"
  availability_zone = "eu-central-1a"
    map_public_ip_on_launch = true
  tags = {
    Name = "Ultrafinance"
  }
}

resource "aws_subnet" "b" {
  vpc_id            = aws_vpc.default.id
  cidr_block        = "10.0.2.0/24"
  availability_zone = "eu-central-1b"
  map_public_ip_on_launch = true

  tags = {
    Name = "Ultrafinance"
  }
}

resource "aws_route_table_association" "a" {
  subnet_id      = aws_subnet.a.id
  route_table_id = aws_route_table.default.id
}

resource "aws_route_table_association" "b" {
    subnet_id      = aws_subnet.b.id
  route_table_id = aws_route_table.default.id
}

resource "aws_route_table" "default" {
  vpc_id = aws_vpc.default.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.default.id
  }

  tags = {
    Name = "Ultrafinance"
  }
}

resource "aws_internet_gateway" "default" {
  vpc_id = aws_vpc.default.id

  tags = {
    Name = "Ultrafinance"
  }
}

resource "aws_route53_zone" "ultrafinance" {
  name = "ultrafinance.app"
}

resource "aws_route53_record" "root" {
  zone_id = aws_route53_zone.ultrafinance.zone_id
  name    = ""
  type = "A"
  alias {
    name = aws_cloudfront_distribution.default.domain_name
    zone_id = aws_cloudfront_distribution.default.hosted_zone_id
    evaluate_target_health = false
  }
}

resource "aws_route53_record" "www" {
  zone_id = aws_route53_zone.ultrafinance.zone_id
  name    = "www"
  type = "A"
  alias {
    name = aws_cloudfront_distribution.default.domain_name
    zone_id = aws_cloudfront_distribution.default.hosted_zone_id
    evaluate_target_health = false
  }
}

resource "aws_ecs_cluster" "default" {
  name = "ultrafinance-production"
}

resource "aws_ecs_task_definition" "web" {
  family                   = "web"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = 1024
  memory                   = 2048
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "ARM64"
  }

  execution_role_arn = aws_iam_role.ecs.arn
  task_role_arn = aws_iam_role.ecs_task_role.arn

  container_definitions = jsonencode([
    {
      name      = "web"
      image     = "${aws_ecr_repository.default.repository_url}:${var.ecr_image_revision}"
      essential = true
      portMappings = [
        {
          containerPort = 3000
          hostPort      = 3000
        }
      ],
      environment : [
        {
          name  = "DATABASE_URL"
          value = "mysql://${aws_rds_cluster.default.master_username}:${random_password.password.result}@${aws_rds_cluster.default.endpoint}/production"
        },
        {
          name  = "API_KEY_SALT"
          value = random_password.api_key_salt.result
        },
        {
          name  = "COOKIE_SECRET"
          value = var.cookie_secret
        },
        {
          name  = "SITE_URL"
          value = "https://ultrafinance.app"
        },
        {
          name  = "NORDIGEN_SECRET_ID"
          value = var.nordigen_secret_id
        },
        {
          name  = "NORDIGEN_SECRET_KEY"
          value = var.nordigen_secret_key
        },
        {
          name = "NTROPY_API_KEY"
          value = var.ntropy_api_key
        }
      ],
      logConfiguration: {
        logDriver = "awslogs"
        options: {
            "awslogs-group" = "ultrafinance/web"
            "awslogs-region" = "eu-central-1"
            "awslogs-stream-prefix" = "container-"
        }
      },
      linuxParameters: {
        initProcessEnabled = true
      }
    }
  ])
}

resource "aws_cloudwatch_log_group" "web" {
  name = "ultrafinance/web"
    retention_in_days = 7
}

resource "aws_iam_role" "ecs" {
  name                = "ultrafinance-ecs"
  managed_policy_arns = ["arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"]
  assume_role_policy = jsonencode({
    "Version" : "2012-10-17",
    "Statement" : [
      {
        "Sid" : "",
        "Effect" : "Allow",
        "Principal" : {
          "Service" : "ecs-tasks.amazonaws.com"
        },
        "Action" : "sts:AssumeRole"
      }
    ]
  })
}

resource "aws_iam_role" "ecs_task_role" {
  name                = "ultrafinance-ecs-task-role"
  assume_role_policy = jsonencode({
    "Version" : "2012-10-17",
    "Statement" : [
      {
        "Sid" : "",
        "Effect" : "Allow",
        "Principal" : {
          "Service" : "ecs-tasks.amazonaws.com"
        },
        "Action" : "sts:AssumeRole"
      }
    ]
  })
  inline_policy {
    name = "ECS-Exec"
    policy = jsonencode({
      "Version" : "2012-10-17",
      "Statement" : [
        {
          "Effect" : "Allow",
          "Action" : [
            "ssmmessages:CreateControlChannel",
            "ssmmessages:CreateDataChannel",
            "ssmmessages:OpenControlChannel",
            "ssmmessages:OpenDataChannel"
          ],
          "Resource" : "*"
        }
      ]
    })
  }
}

resource "aws_ecs_service" "web" {
  name            = "web"
  cluster         = aws_ecs_cluster.default.id
  task_definition = aws_ecs_task_definition.web.arn
  desired_count   = 1
  launch_type     = "FARGATE"
  enable_execute_command = true
  network_configuration {
    subnets = [aws_subnet.a.id, aws_subnet.b.id]
    assign_public_ip = true
    security_groups = [ aws_security_group.ecs.id ]
  }
  load_balancer {
    target_group_arn = aws_lb_target_group.default.arn
    container_name   = "web"
    container_port   = 3000
  }
}

resource "aws_ecr_repository" "default" {
  name = "ultrafinance"
}

resource "aws_lb" "default" {
  name               = "ultrafinance"
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.lb.id]
  subnets            = [aws_subnet.a.id, aws_subnet.b.id]
}

resource "aws_lb_listener" "default" {
  load_balancer_arn = aws_lb.default.arn
  port              = "443"
  protocol          = "HTTPS"
  certificate_arn = aws_acm_certificate.local.arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.default.arn
  }
}

resource "aws_security_group" "lb" {
  name   = "ultrafinance-lb"
  vpc_id = aws_vpc.default.id

  ingress {
    description = "ALB"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port        = 0
    to_port          = 0
    protocol         = "-1"
    cidr_blocks      = ["0.0.0.0/0"]
    ipv6_cidr_blocks = ["::/0"]
  }
}

resource "aws_security_group" "ecs" {
  name   = "ultrafinance-ecs"
  vpc_id = aws_vpc.default.id

  ingress {
    description = "HTTP from ECS"
    from_port   = 0
    to_port     = 3001
    protocol    = "tcp"
   security_groups = [aws_security_group.lb.id]
  }

  egress {
    from_port        = 0
    to_port          = 0
    protocol         = "-1"
    cidr_blocks      = ["0.0.0.0/0"]
    ipv6_cidr_blocks = ["::/0"]
  }
}

resource "aws_security_group" "rds" {
  name   = "ultrafinance-rds"
  vpc_id = aws_vpc.default.id

  ingress {
    description = "MySQL from ECS"
    from_port   = 3306
    to_port     = 3306
    protocol    = "tcp"
   security_groups = [aws_security_group.ecs.id]
  }
}

resource "aws_lb_target_group" "default" {
  name     = "web"
  port     = 80
  protocol = "HTTP"
  vpc_id   = aws_vpc.default.id
  target_type = "ip"
  health_check {
    enabled = true
    healthy_threshold = 2
    interval = 5
    timeout = 4
    matcher = "200"
    path = "/"
    protocol = "HTTP"
  }
}

resource "random_password" "password" {
  length           = 16
  special          = true
  override_special = "!#$%&*()-_=+[]{}<>:?"
}

resource "random_password" "api_key_salt" {
  length           = 32
  special          = true
  override_special = "!#$%&*()-_=+[]{}<>:?"
}

resource "aws_db_subnet_group" "default" {
  name       = "ultrafinance"
  subnet_ids = [aws_subnet.a.id, aws_subnet.b.id]

  tags = {
    Name = "Ultrafinance"
  }
}

resource "aws_rds_cluster" "default" {
  cluster_identifier = "ultrafinance"
  engine             = "aurora-mysql"
  engine_mode        = "provisioned"
  engine_version     = "8.0.mysql_aurora.3.02.2"
  database_name      = "production"
  master_username    = "ultrafinance"
  master_password    = random_password.password.result
  storage_encrypted = true
  skip_final_snapshot = true
  db_subnet_group_name = "ultrafinance"
  serverlessv2_scaling_configuration {
    max_capacity = 1.0
    min_capacity = 0.5
  }
  vpc_security_group_ids = [aws_security_group.rds.id]
}

resource "aws_rds_cluster_instance" "default" {
  publicly_accessible = true
  cluster_identifier = aws_rds_cluster.default.id
  instance_class     = "db.serverless"
  engine             = aws_rds_cluster.default.engine
  engine_version     = aws_rds_cluster.default.engine_version
}

resource "aws_cloudfront_distribution" "default" {
  aliases = ["ultrafinance.app", "www.ultrafinance.app"]
  enabled = true
  http_version = "http2and3"
  origin {
    origin_id   = "web"
    domain_name = aws_lb.default.dns_name
    custom_origin_config {
      http_port              = 80
      https_port             = 443
      origin_protocol_policy = "https-only"
      origin_ssl_protocols   = ["TLSv1.2"]
      origin_read_timeout    = 60
    }
  }
  default_cache_behavior {
    allowed_methods  = ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"]
    cached_methods   = ["GET", "HEAD"]
    target_origin_id = "web"
    forwarded_values {
      query_string = true

      cookies {
        forward           = "whitelist"
        whitelisted_names = ["id"]
      }

      headers = ["Host", "Authorization"]
    }

    viewer_protocol_policy = "redirect-to-https"
    min_ttl                = 0
    default_ttl            = 3600
    max_ttl                = 86400
  }
  restrictions {
    geo_restriction {
      restriction_type = "none"

    }
  }
  viewer_certificate {
    acm_certificate_arn = aws_acm_certificate.default.arn
    minimum_protocol_version = "TLSv1.2_2021"
    ssl_support_method = "sni-only"
  }
}

resource "aws_acm_certificate" "default" {
  provider = aws.us-east-1
  domain_name       = "ultrafinance.app"
  subject_alternative_names = [ "www.ultrafinance.app"]
  validation_method = "DNS"
}

resource "aws_acm_certificate" "local" {
  domain_name       = "ultrafinance.app"
  subject_alternative_names = [ "www.ultrafinance.app"]
  validation_method = "DNS"
}

output "deployed_revision" {
  value = var.ecr_image_revision
}

output "aws_cloudwatch_log_group_web" {
  value = aws_cloudwatch_log_group.web.name
}
