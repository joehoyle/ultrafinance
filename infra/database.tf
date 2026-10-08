# Opt-in infrastructure: provision the database before cutting over the live alias.
data "aws_availability_zones" "database" {
  count = var.enable_aurora ? 1 : 0
  state = "available"
}

resource "aws_vpc" "database" {
  count                = var.enable_aurora ? 1 : 0
  cidr_block           = "10.42.0.0/16"
  enable_dns_support   = true
  enable_dns_hostnames = true
  tags                 = { Name = "${var.name}-database" }
}

resource "aws_subnet" "database" {
  count             = var.enable_aurora ? 2 : 0
  vpc_id            = aws_vpc.database[0].id
  cidr_block        = cidrsubnet(aws_vpc.database[0].cidr_block, 8, count.index)
  availability_zone = data.aws_availability_zones.database[0].names[count.index]
  tags              = { Name = "${var.name}-private-${count.index + 1}" }
}

resource "aws_subnet" "nat" {
  count             = var.enable_aurora ? 1 : 0
  vpc_id            = aws_vpc.database[0].id
  cidr_block        = cidrsubnet(aws_vpc.database[0].cidr_block, 8, 2)
  availability_zone = data.aws_availability_zones.database[0].names[0]
  tags              = { Name = "${var.name}-nat" }
}

resource "aws_internet_gateway" "database" {
  count  = var.enable_aurora ? 1 : 0
  vpc_id = aws_vpc.database[0].id
}

resource "aws_route_table" "public" {
  count  = var.enable_aurora ? 1 : 0
  vpc_id = aws_vpc.database[0].id
}

resource "aws_route" "internet" {
  count                  = var.enable_aurora ? 1 : 0
  route_table_id         = aws_route_table.public[0].id
  destination_cidr_block = "0.0.0.0/0"
  gateway_id             = aws_internet_gateway.database[0].id
}

resource "aws_route_table_association" "nat" {
  count          = var.enable_aurora ? 1 : 0
  subnet_id      = aws_subnet.nat[0].id
  route_table_id = aws_route_table.public[0].id
}

resource "aws_eip" "nat" {
  count  = var.enable_aurora ? 1 : 0
  domain = "vpc"
}

# One managed gateway keeps idle costs low; both AZs depend on its availability.
resource "aws_nat_gateway" "database" {
  count         = var.enable_aurora ? 1 : 0
  allocation_id = aws_eip.nat[0].id
  subnet_id     = aws_subnet.nat[0].id
  depends_on    = [aws_internet_gateway.database, aws_route.internet, aws_route_table_association.nat]
}

resource "aws_route_table" "private" {
  count  = var.enable_aurora ? 1 : 0
  vpc_id = aws_vpc.database[0].id
}

resource "aws_route" "nat" {
  count                  = var.enable_aurora ? 1 : 0
  route_table_id         = aws_route_table.private[0].id
  destination_cidr_block = "0.0.0.0/0"
  nat_gateway_id         = aws_nat_gateway.database[0].id
}

resource "aws_route_table_association" "database" {
  count          = var.enable_aurora ? 2 : 0
  subnet_id      = aws_subnet.database[count.index].id
  route_table_id = aws_route_table.private[0].id
}

resource "aws_security_group" "database_client" {
  count       = var.enable_aurora ? 1 : 0
  name_prefix = "${var.name}-database-client-"
  description = "Lambda and authorized migration clients"
  vpc_id      = aws_vpc.database[0].id
}

resource "aws_security_group" "database" {
  count       = var.enable_aurora ? 1 : 0
  name_prefix = "${var.name}-postgres-"
  description = "Private Aurora PostgreSQL"
  vpc_id      = aws_vpc.database[0].id
}

resource "aws_vpc_security_group_ingress_rule" "postgres" {
  count                        = var.enable_aurora ? 1 : 0
  security_group_id            = aws_security_group.database[0].id
  referenced_security_group_id = aws_security_group.database_client[0].id
  ip_protocol                  = "tcp"
  from_port                    = 5432
  to_port                      = 5432
}

resource "aws_vpc_security_group_egress_rule" "postgres" {
  count                        = var.enable_aurora ? 1 : 0
  security_group_id            = aws_security_group.database_client[0].id
  referenced_security_group_id = aws_security_group.database[0].id
  ip_protocol                  = "tcp"
  from_port                    = 5432
  to_port                      = 5432
}

resource "aws_vpc_security_group_egress_rule" "https" {
  count             = var.enable_aurora ? 1 : 0
  security_group_id = aws_security_group.database_client[0].id
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "tcp"
  from_port         = 443
  to_port           = 443
}

resource "aws_db_subnet_group" "database" {
  count      = var.enable_aurora ? 1 : 0
  name       = "${var.name}-postgres"
  subnet_ids = aws_subnet.database[*].id
}

resource "aws_rds_cluster_parameter_group" "database" {
  count       = var.enable_aurora ? 1 : 0
  name_prefix = "${var.name}-postgres-"
  family      = "aurora-postgresql17"
  parameter {
    name         = "rds.force_ssl"
    value        = "1"
    apply_method = "pending-reboot"
  }
}

resource "aws_rds_cluster" "database" {
  count                           = var.enable_aurora ? 1 : 0
  cluster_identifier              = "${var.name}-postgres"
  engine                          = "aurora-postgresql"
  engine_mode                     = "provisioned" # Serverless v2 uses provisioned engine mode.
  engine_version                  = var.aurora_engine_version
  database_name                   = "ultrafinance"
  master_username                 = "ultrafinance_admin"
  manage_master_user_password     = true
  db_subnet_group_name            = aws_db_subnet_group.database[0].name
  db_cluster_parameter_group_name = aws_rds_cluster_parameter_group.database[0].name
  vpc_security_group_ids          = [aws_security_group.database[0].id]
  storage_encrypted               = true
  backup_retention_period         = 7
  deletion_protection             = true
  skip_final_snapshot             = false
  final_snapshot_identifier       = "${var.name}-postgres-final"
  copy_tags_to_snapshot           = true
  serverlessv2_scaling_configuration {
    min_capacity             = var.aurora_min_acu
    max_capacity             = var.aurora_max_acu
    seconds_until_auto_pause = var.aurora_min_acu == 0 ? 300 : null
  }
}

resource "aws_rds_cluster_instance" "database" {
  count                = var.enable_aurora ? 1 : 0
  identifier           = "${var.name}-postgres-writer"
  cluster_identifier   = aws_rds_cluster.database[0].id
  engine               = aws_rds_cluster.database[0].engine
  engine_version       = aws_rds_cluster.database[0].engine_version
  instance_class       = "db.serverless"
  publicly_accessible  = false
  db_subnet_group_name = aws_db_subnet_group.database[0].name
}

resource "aws_iam_role_policy_attachment" "lambda_vpc" {
  count      = var.enable_aurora && local.deploy_app ? 1 : 0
  role       = aws_iam_role.lambda[0].name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaVPCAccessExecutionRole"
}
