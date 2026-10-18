# Immutable image assets bypass Lambda and Aurora entirely.
resource "aws_s3_bucket" "logos" {
  count         = local.deploy_app ? 1 : 0
  bucket_prefix = "${var.name}-logos-"
  force_destroy = false
}

resource "aws_s3_bucket_public_access_block" "logos" {
  count                   = local.deploy_app ? 1 : 0
  bucket                  = aws_s3_bucket.logos[0].id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "logos" {
  count  = local.deploy_app ? 1 : 0
  bucket = aws_s3_bucket.logos[0].id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_cloudfront_origin_access_control" "logos" {
  count                             = local.deploy_app ? 1 : 0
  name                              = "${var.name}-logos"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

resource "aws_s3_bucket_policy" "logos" {
  count  = local.deploy_app ? 1 : 0
  bucket = aws_s3_bucket.logos[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "cloudfront.amazonaws.com" }
      Action    = "s3:GetObject"
      Resource  = "${aws_s3_bucket.logos[0].arn}/logos/*"
      Condition = { StringEquals = { "AWS:SourceArn" = aws_cloudfront_distribution.app[0].arn } }
    }]
  })
}

# Originals can contain active SVG. Force download and sandbox every asset;
# selected display URLs are always raster WebP images.
resource "aws_cloudfront_response_headers_policy" "logos" {
  count = local.deploy_app ? 1 : 0
  name  = "${var.name}-logos"
  security_headers_config {
    content_type_options {
      override = true
    }
    content_security_policy {
      content_security_policy = "default-src 'none'; sandbox"
      override                = true
    }
  }
}

resource "aws_iam_role_policy" "cli_logos" {
  count = local.cli_enabled ? 1 : 0
  role  = aws_iam_role.cli_task[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["s3:PutObject"]
      Resource = "${aws_s3_bucket.logos[0].arn}/logos/*"
    }]
  })
}

output "logo_bucket" {
  value = local.deploy_app ? aws_s3_bucket.logos[0].id : null
}
