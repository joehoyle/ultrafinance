locals {
  deploy_app = var.image_uri != null
}

resource "aws_ecr_repository" "app" {
  name                 = var.name
  image_tag_mutability = "IMMUTABLE"
  image_scanning_configuration {
    scan_on_push = true
  }
  encryption_configuration {
    encryption_type = "AES256"
  }
}

resource "aws_iam_role" "lambda" {
  count = local.deploy_app ? 1 : 0
  name  = "${var.name}-lambda"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow", Principal = { Service = "lambda.amazonaws.com" }, Action = "sts:AssumeRole"
    }]
  })
}

resource "aws_cloudwatch_log_group" "lambda" {
  count             = local.deploy_app ? 1 : 0
  name              = "/aws/lambda/${var.name}"
  retention_in_days = 14
}

resource "aws_iam_role_policy" "logs" {
  count = local.deploy_app ? 1 : 0
  role  = aws_iam_role.lambda[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow", Action = ["logs:CreateLogStream", "logs:PutLogEvents"],
      Resource = "${aws_cloudwatch_log_group.lambda[0].arn}:*"
    }]
  })
}

resource "aws_lambda_function" "app" {
  count         = local.deploy_app ? 1 : 0
  function_name = var.name
  role          = aws_iam_role.lambda[0].arn
  package_type  = "Image"
  image_uri     = var.image_uri
  architectures = ["arm64"]
  publish       = true
  memory_size   = 1024
  timeout       = 30
  # No VPC/NAT needed: Jev calls use Lambda's normal outbound network.
  environment {
    variables = {
      TYPESAFE_API_KEY             = var.typesafe_api_key
      JEV_MODEL                    = var.jev_model
      ULTRAFINANCE_MATCH_THRESHOLD = tostring(var.match_threshold)
    }
  }
  # Release tooling owns image updates; Tofu still owns runtime configuration.
  lifecycle {
    ignore_changes = [image_uri]
  }
  depends_on = [aws_iam_role_policy.logs]
}

# Stable endpoint: releases publish a version and promote this alias only after checks.
resource "aws_lambda_alias" "live" {
  count            = local.deploy_app ? 1 : 0
  name             = "live"
  function_name    = aws_lambda_function.app[0].function_name
  function_version = aws_lambda_function.app[0].version
  lifecycle {
    ignore_changes = [function_version, routing_config]
  }
}

# Public function URL; ordinary JSON POST requests need no client credentials.
resource "aws_lambda_function_url" "app" {
  count              = local.deploy_app ? 1 : 0
  function_name      = aws_lambda_function.app[0].function_name
  qualifier          = aws_lambda_alias.live[0].name
  authorization_type = "NONE"
  invoke_mode        = "BUFFERED"
}

# New function URLs require both invocation permissions, including NONE auth.
resource "aws_lambda_permission" "function_url" {
  count                  = local.deploy_app ? 1 : 0
  statement_id           = "AllowPublicFunctionUrl"
  action                 = "lambda:InvokeFunctionUrl"
  function_name          = aws_lambda_function.app[0].function_name
  qualifier              = aws_lambda_alias.live[0].name
  principal              = "*"
  function_url_auth_type = "NONE"
}

resource "aws_lambda_permission" "function_url_invoke" {
  count                    = local.deploy_app ? 1 : 0
  statement_id             = "AllowInvokeViaFunctionUrl"
  action                   = "lambda:InvokeFunction"
  function_name            = aws_lambda_function.app[0].function_name
  qualifier                = aws_lambda_alias.live[0].name
  principal                = "*"
  invoked_via_function_url = true
}

resource "aws_cloudfront_distribution" "app" {
  count           = local.deploy_app ? 1 : 0
  enabled         = true
  comment         = var.name
  aliases         = var.domain_name == null ? [] : [var.domain_name]
  is_ipv6_enabled = true
  # Keep the AWS hostname available alongside the optional custom domain.
  price_class = "PriceClass_100"
  origin {
    domain_name = trimsuffix(trimprefix(aws_lambda_function_url.app[0].function_url, "https://"), "/")
    origin_id   = "api"
    custom_origin_config {
      http_port              = 80
      https_port             = 443
      origin_protocol_policy = "https-only"
      origin_ssl_protocols   = ["TLSv1.2"]
      origin_read_timeout    = 35
    }
  }
  default_cache_behavior {
    target_origin_id       = "api"
    viewer_protocol_policy = "https-only"
    allowed_methods        = ["GET", "HEAD", "OPTIONS", "PUT", "POST", "PATCH", "DELETE"]
    cached_methods         = ["GET", "HEAD"]
    # AWS managed CachingDisabled and AllViewerExceptHostHeader policies.
    cache_policy_id          = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"
    origin_request_policy_id = "b689b0a8-53d0-40ab-baf2-68738e2966ac"
    compress                 = true
  }
  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }
  viewer_certificate {
    cloudfront_default_certificate = var.domain_name == null
    acm_certificate_arn            = var.domain_name == null ? null : aws_acm_certificate_validation.site[0].certificate_arn
    ssl_support_method             = var.domain_name == null ? null : "sni-only"
    minimum_protocol_version       = var.domain_name == null ? "TLSv1" : "TLSv1.2_2021"
  }
  depends_on = [aws_lambda_permission.function_url, aws_lambda_permission.function_url_invoke]
}
