locals {
  deploy_domain = local.deploy_app && var.domain_name != null
}

# Import the existing zone; its email and other records remain independently managed.
resource "aws_route53_zone" "site" {
  count   = var.domain_name == null ? 0 : 1
  name    = var.domain_name
  comment = "Managed by Terraform"
  lifecycle {
    prevent_destroy = true
  }
}

# CloudFront requires ACM certificates in us-east-1.
resource "aws_acm_certificate" "site" {
  count             = local.deploy_domain ? 1 : 0
  provider          = aws.us_east_1
  domain_name       = var.domain_name
  validation_method = "DNS"
  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_route53_record" "certificate" {
  count           = local.deploy_domain ? 1 : 0
  zone_id         = aws_route53_zone.site[0].zone_id
  name            = one(aws_acm_certificate.site[0].domain_validation_options).resource_record_name
  type            = one(aws_acm_certificate.site[0].domain_validation_options).resource_record_type
  records         = [one(aws_acm_certificate.site[0].domain_validation_options).resource_record_value]
  ttl             = 60
  allow_overwrite = true
}

resource "aws_acm_certificate_validation" "site" {
  count                   = local.deploy_domain ? 1 : 0
  provider                = aws.us_east_1
  certificate_arn         = aws_acm_certificate.site[0].arn
  validation_record_fqdns = [for record in aws_route53_record.certificate : record.fqdn]
}

resource "aws_route53_record" "site" {
  for_each = local.deploy_domain ? toset(["A", "AAAA"]) : toset([])
  zone_id  = aws_route53_zone.site[0].zone_id
  name     = var.domain_name
  type     = each.value
  alias {
    name                   = aws_cloudfront_distribution.app[0].domain_name
    zone_id                = aws_cloudfront_distribution.app[0].hosted_zone_id
    evaluate_target_health = false
  }
}
