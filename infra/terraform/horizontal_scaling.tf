# Horizontal Scaling Infrastructure (Issue #397)
# Provisions EKS cluster with node auto-scaling groups per region.
# Replicable across staging and production via workspace variables.

variable "environment" {
  description = "Deployment environment (staging | production)"
  type        = string
}

variable "region" {
  description = "AWS region"
  type        = string
}

variable "cluster_name" {
  description = "EKS cluster name"
  type        = string
  default     = "aframp-cluster"
}

variable "node_instance_type" {
  description = "EC2 instance type for worker nodes"
  type        = string
  default     = "t3.medium"
}

variable "node_min_size" {
  type    = number
  default = 2
}

variable "node_max_size" {
  type    = number
  default = 20
}

variable "node_desired_size" {
  type    = number
  default = 3
}

# ── EKS Cluster ──────────────────────────────────────────────────────────────

resource "aws_eks_cluster" "aframp" {
  name     = "${var.cluster_name}-${var.environment}"
  role_arn = aws_iam_role.eks_cluster.arn
  version  = "1.29"

  vpc_config {
    subnet_ids              = aws_subnet.private[*].id
    endpoint_private_access = true
    endpoint_public_access  = false
  }

  tags = {
    Environment = var.environment
    ManagedBy   = "terraform"
  }
}

# ── Managed Node Group with Auto-Scaling ─────────────────────────────────────

resource "aws_eks_node_group" "aframp_workers" {
  cluster_name    = aws_eks_cluster.aframp.name
  node_group_name = "aframp-workers-${var.environment}"
  node_role_arn   = aws_iam_role.eks_node.arn
  subnet_ids      = aws_subnet.private[*].id
  instance_types  = [var.node_instance_type]

  scaling_config {
    min_size     = var.node_min_size
    max_size     = var.node_max_size
    desired_size = var.node_desired_size
  }

  update_config {
    max_unavailable = 1
  }

  labels = {
    environment = var.environment
    role        = "worker"
  }

  tags = {
    Environment                                                   = var.environment
    "k8s.io/cluster-autoscaler/${aws_eks_cluster.aframp.name}"   = "owned"
    "k8s.io/cluster-autoscaler/enabled"                          = "true"
  }
}

# ── Cluster Autoscaler IAM ────────────────────────────────────────────────────

resource "aws_iam_role" "eks_cluster" {
  name = "aframp-eks-cluster-${var.environment}"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "eks.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy_attachment" "eks_cluster_policy" {
  role       = aws_iam_role.eks_cluster.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKSClusterPolicy"
}

resource "aws_iam_role" "eks_node" {
  name = "aframp-eks-node-${var.environment}"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy_attachment" "eks_worker_node_policy" {
  role       = aws_iam_role.eks_node.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKSWorkerNodePolicy"
}

resource "aws_iam_role_policy_attachment" "eks_cni_policy" {
  role       = aws_iam_role.eks_node.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy"
}

resource "aws_iam_role_policy_attachment" "eks_ecr_readonly" {
  role       = aws_iam_role.eks_node.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonEC2ContainerRegistryReadOnly"
}

# ── Cluster Autoscaler policy (scales node groups based on pending pods) ──────

resource "aws_iam_policy" "cluster_autoscaler" {
  name = "aframp-cluster-autoscaler-${var.environment}"
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "autoscaling:DescribeAutoScalingGroups",
        "autoscaling:DescribeAutoScalingInstances",
        "autoscaling:DescribeLaunchConfigurations",
        "autoscaling:DescribeScalingActivities",
        "autoscaling:SetDesiredCapacity",
        "autoscaling:TerminateInstanceInAutoScalingGroup",
        "ec2:DescribeImages",
        "ec2:DescribeInstanceTypes",
        "ec2:DescribeLaunchTemplateVersions",
        "ec2:GetInstanceTypesFromInstanceRequirements",
        "eks:DescribeNodegroup"
      ]
      Resource = "*"
    }]
  })
}

# ── Outputs ───────────────────────────────────────────────────────────────────

output "cluster_endpoint" {
  value = aws_eks_cluster.aframp.endpoint
}

output "cluster_name" {
  value = aws_eks_cluster.aframp.name
}
