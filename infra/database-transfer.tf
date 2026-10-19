# Short-lived logical archives. This bucket is never exposed through CloudFront.
resource "aws_s3_bucket" "database_transfer" {
  count         = local.cli_enabled ? 1 : 0
  bucket_prefix = "${substr(var.name, 0, 20)}-db-transfer-"
  force_destroy = false
}

resource "aws_s3_bucket_public_access_block" "database_transfer" {
  count                   = local.cli_enabled ? 1 : 0
  bucket                  = aws_s3_bucket.database_transfer[0].id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "database_transfer" {
  count  = local.cli_enabled ? 1 : 0
  bucket = aws_s3_bucket.database_transfer[0].id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

# Cleanup normally deletes the unique invocation prefix immediately. Expiration
# handles abandoned transfers; incomplete multipart uploads are also reclaimed.
resource "aws_s3_bucket_lifecycle_configuration" "database_transfer" {
  count  = local.cli_enabled ? 1 : 0
  bucket = aws_s3_bucket.database_transfer[0].id
  rule {
    id     = "expire-transfers"
    status = "Enabled"
    filter {}
    expiration {
      days = 1
    }
    abort_incomplete_multipart_upload {
      days_after_initiation = 1
    }
  }
}

# Download from S3 directly inside AWS rather than through the NAT gateway.
resource "aws_vpc_endpoint" "database_s3" {
  count             = local.cli_enabled ? 1 : 0
  vpc_id            = aws_vpc.database[0].id
  service_name      = "com.amazonaws.${var.aws_region}.s3"
  vpc_endpoint_type = "Gateway"
  route_table_ids   = [aws_route_table.private[0].id]
}
