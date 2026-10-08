terraform {
  required_version = ">= 1.10, < 2.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "6.68.0"
    }
  }
}

provider "aws" {
  profile             = var.aws_use_cli_credentials ? null : var.aws_profile
  region              = var.aws_region
  allowed_account_ids = var.aws_account_id == null ? null : [var.aws_account_id]
  default_tags {
    tags = { Project = var.name, ManagedBy = "OpenTofu" }
  }
}

provider "aws" {
  alias               = "us_east_1"
  profile             = var.aws_use_cli_credentials ? null : var.aws_profile
  region              = "us-east-1"
  allowed_account_ids = var.aws_account_id == null ? null : [var.aws_account_id]
  default_tags {
    tags = { Project = var.name, ManagedBy = "OpenTofu" }
  }
}
