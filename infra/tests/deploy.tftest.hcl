mock_provider "aws" {
  alias = "us_east_1"
}

run "aurora_bootstrap" {
  command = plan
  variables {
    domain_name       = null
    image_uri         = "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    enable_aurora     = true
    database_url      = null
    github_repository = null
  }
  assert {
    condition     = aws_rds_cluster.database[0].storage_encrypted && aws_rds_cluster.database[0].deletion_protection && aws_rds_cluster.database[0].manage_master_user_password && !aws_rds_cluster_instance.database[0].publicly_accessible
    error_message = "Aurora must be private, encrypted, protected, and use an RDS-managed admin secret."
  }
  assert {
    condition     = length(aws_subnet.database) == 2 && length(aws_nat_gateway.database) == 1 && aws_route.nat[0].nat_gateway_id == aws_nat_gateway.database[0].id
    error_message = "Private subnets in two AZs must share one managed NAT gateway."
  }
  assert {
    condition     = length(aws_lambda_function.app[0].vpc_config) == 0
    error_message = "Provisioning Aurora must not move Lambda before the runtime URL is configured."
  }
}

run "aurora_cutover" {
  command = plan
  variables {
    domain_name       = null
    image_uri         = "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    enable_aurora     = true
    database_url      = "postgresql://runtime:example@database.example/ultrafinance?sslmode=require"
    github_repository = null
    aurora_min_acu    = 0
  }
  assert {
    condition     = aws_lambda_function.app[0].vpc_config[0].subnet_ids == toset(aws_subnet.database[*].id) && aws_lambda_function.app[0].vpc_config[0].security_group_ids == toset([aws_security_group.database_client[0].id])
    error_message = "At cutover Lambda must use the private subnets and the authorized database-client security group."
  }
  assert {
    condition     = aws_rds_cluster.database[0].serverlessv2_scaling_configuration[0].min_capacity == 0 && aws_rds_cluster.database[0].serverlessv2_scaling_configuration[0].seconds_until_auto_pause == 300
    error_message = "A zero minimum must enable auto-pause after five minutes."
  }
  assert {
    condition     = aws_vpc_security_group_ingress_rule.postgres[0].referenced_security_group_id == aws_security_group.database_client[0].id && aws_vpc_security_group_ingress_rule.postgres[0].from_port == 5432 && aws_vpc_security_group_egress_rule.https[0].to_port == 443
    error_message = "Only authorized clients may reach PostgreSQL; outbound HTTPS must remain available."
  }
}

run "cli_tasks" {
  command = plan
  variables {
    domain_name       = null
    image_uri         = "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    enable_aurora     = true
    database_url      = null
    github_repository = null
  }
  assert {
    condition     = aws_ecs_task_definition.cli[0].runtime_platform[0].cpu_architecture == "ARM64" && aws_ecs_task_definition.cli[0].network_mode == "awsvpc" && aws_ecs_task_definition.cli[0].requires_compatibilities == toset(["FARGATE"])
    error_message = "CLI tasks must run as ARM64 Fargate tasks inside the VPC."
  }
  assert {
    condition     = jsondecode(aws_ecs_task_definition.cli[0].container_definitions)[1].image == var.image_uri && jsondecode(aws_ecs_task_definition.cli[0].container_definitions)[1].entryPoint == ["/usr/local/bin/ultrafinance"]
    error_message = "The CLI must reuse the immutable application image and bypass the Lambda entrypoint."
  }
  assert {
    condition     = jsondecode(aws_ecs_task_definition.cli[0].container_definitions)[1].dependsOn[0].condition == "SUCCESS" && !jsondecode(aws_ecs_task_definition.cli[0].container_definitions)[0].essential
    error_message = "The CLI must wait for a successful download; the input helper must be nonessential."
  }
  assert {
    condition     = jsondecode(aws_ecs_task_definition.cli[0].container_definitions)[1].secrets[0].valueFrom == aws_secretsmanager_secret.cli_database_url[0].arn && length(aws_lambda_function.app[0].vpc_config) == 0
    error_message = "Import credentials must come from their own secret, without cutting over Lambda."
  }
  assert {
    condition     = aws_s3_bucket_public_access_block.cli_inputs[0].block_public_acls && aws_s3_bucket_public_access_block.cli_inputs[0].block_public_policy && aws_s3_bucket_public_access_block.cli_inputs[0].ignore_public_acls && aws_s3_bucket_public_access_block.cli_inputs[0].restrict_public_buckets
    error_message = "Staged catalog inputs must stay private."
  }
}

mock_provider "aws" {
  mock_data "aws_availability_zones" {
    defaults = { names = ["ca-central-1a", "ca-central-1b"] }
  }
  mock_resource "aws_rds_cluster" {
    defaults = {
      master_user_secret = [{
        secret_arn    = "arn:aws:secretsmanager:ca-central-1:123456789012:secret:database-example"
        secret_status = "active"
        kms_key_id    = "arn:aws:kms:ca-central-1:123456789012:key/example"
      }]
    }
  }
  mock_resource "aws_iam_role" {
    defaults = {
      arn = "arn:aws:iam::123456789012:role/ultrafinance-lambda"
    }
  }
  mock_resource "aws_lambda_function" {
    defaults = {
      arn     = "arn:aws:lambda:ca-central-1:123456789012:function:ultrafinance"
      version = "1"
    }
  }
}

run "repository_bootstrap" {
  command = plan
  variables {
    domain_name              = null
    image_uri                = null
    github_repository        = null
    github_oidc_provider_arn = null
  }
  assert {
    condition     = length(aws_lambda_function.app) == 0 && length(aws_iam_role.deploy) == 0
    error_message = "Default bootstrap must not create an application or an unconfigured CI role."
  }
}

run "alias_and_oidc" {
  command = plan
  variables {
    domain_name          = null
    image_uri            = "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    github_repository    = "joehoyle/ultrafinance"
    github_owner_id      = "161683"
    github_repository_id = "1410780058"
  }
  assert {
    condition     = aws_lambda_function_url.app[0].qualifier == "live" && aws_lambda_function_url.app[0].authorization_type == "NONE"
    error_message = "The public endpoint must target the stable live alias."
  }
  assert {
    condition     = aws_lambda_permission.function_url[0].qualifier == "live" && aws_lambda_permission.function_url_invoke[0].qualifier == "live" && aws_lambda_permission.function_url_invoke[0].invoked_via_function_url
    error_message = "Public invocation permissions must be limited to the live function URL."
  }
  assert {
    condition     = toset(jsondecode(aws_iam_role.deploy[0].assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]) == toset(["repo:joehoyle/ultrafinance:ref:refs/heads/main", "repo:joehoyle@161683/ultrafinance@1410780058:ref:refs/heads/main"])
    error_message = "OIDC must trust only this repository's main branch, with exact subjects."
  }
}
