use anyhow::{bail, Result};
use chrono::Local;
use clap::Subcommand;
use rusqlite::{params, Connection, OptionalExtension};

use crate::{config::Config, creds, utils};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum CloudAction {
    /// Provision an ephemeral VM
    Up {
        /// Cloud provider: do | hetzner | aws | azure | gcp
        #[arg(long, default_value = "do")]
        provider: String,
        /// Instance size (provider-specific slug)
        #[arg(long)]
        size: Option<String>,
        /// Region / zone slug
        #[arg(long)]
        region: Option<String>,
        /// OS image slug (GCP: project/family)
        #[arg(long)]
        image: Option<String>,
        /// VM name (auto-generated if omitted)
        #[arg(long)]
        name: Option<String>,
    },
    /// List VMs provisioned by dclaw
    Ls,
    /// Destroy a VM by name or local ID
    Down {
        /// VM name or local ID shown by `dclaw cloud ls`
        vm: String,
    },
    /// Print the SSH command for a VM
    Ssh {
        /// VM name or local ID
        vm: String,
    },
    /// Natural language query — dclaw cloud "<what you want to do>"
    #[command(external_subcommand)]
    Nl(Vec<String>),
}

// ── Constants ─────────────────────────────────────────────────────────────────

const CLOUD_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS cloud_vms (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        provider   TEXT NOT NULL,
        vm_id      TEXT NOT NULL,
        name       TEXT NOT NULL,
        region     TEXT NOT NULL,
        size       TEXT NOT NULL,
        ip         TEXT,
        status     TEXT NOT NULL DEFAULT 'provisioning',
        created_at TEXT NOT NULL
    );
";

const SELECT_ALL: &str =
    "SELECT id,provider,vm_id,name,region,size,ip,status,created_at FROM cloud_vms";

// ── Value objects ─────────────────────────────────────────────────────────────

struct VmSpec {
    name: String,
    size: String,
    region: String,
    image: String,
}

pub struct VmRecord {
    pub id: i64,
    pub provider: String,
    pub vm_id: String,
    pub name: String,
    pub region: String,
    pub size: String,
    pub ip: Option<String>,
    pub status: String,
    pub created_at: String,
}

// ── Provider enum ─────────────────────────────────────────────────────────────

pub enum Provider {
    DigitalOcean,
    Hetzner,
    Aws,
    Azure,
    Gcp,
}

impl Provider {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "do" | "digitalocean" => Ok(Self::DigitalOcean),
            "hetzner" | "hz" => Ok(Self::Hetzner),
            "aws" | "amazon" => Ok(Self::Aws),
            "azure" | "az" => Ok(Self::Azure),
            "gcp" | "google" => Ok(Self::Gcp),
            _ => bail!("Unknown provider `{s}`. Supported: do, hetzner, aws, azure, gcp"),
        }
    }

    /// DO and Hetzner use a Bearer token from keychain; CLI-based providers handle auth themselves.
    pub fn requires_token(&self) -> bool {
        matches!(self, Self::DigitalOcean | Self::Hetzner)
    }

    pub fn keychain_name(&self) -> &'static str {
        match self {
            Self::DigitalOcean => "do-token",
            Self::Hetzner => "hetzner-token",
            Self::Aws => "aws",
            Self::Azure => "azure",
            Self::Gcp => "gcp",
        }
    }

    pub fn default_size(&self) -> &'static str {
        match self {
            Self::DigitalOcean => "s-1vcpu-1gb",
            Self::Hetzner => "cx11",
            Self::Aws => "t3.micro",
            Self::Azure => "Standard_B1s",
            Self::Gcp => "e2-micro",
        }
    }

    pub fn default_region(&self) -> &'static str {
        match self {
            Self::DigitalOcean => "nyc3",
            Self::Hetzner => "nbg1",
            Self::Aws => "us-east-1",
            Self::Azure => "eastus",
            Self::Gcp => "us-central1-a",
        }
    }

    pub fn default_image(&self) -> &'static str {
        match self {
            Self::DigitalOcean => "ubuntu-22-04-x64",
            Self::Hetzner => "ubuntu-22.04",
            Self::Aws => "ami-0c02fb55956c7d316",
            Self::Azure => "Ubuntu2204",
            Self::Gcp => "debian-cloud/debian-11",
        }
    }

    /// Default SSH username for the provider's base image.
    pub fn default_ssh_user(&self) -> &'static str {
        match self {
            Self::DigitalOcean => "root",
            Self::Hetzner => "root",
            Self::Aws => "ec2-user",
            Self::Azure => "azureuser",
            Self::Gcp => "user",
        }
    }
}

// ── SQLite VM store ───────────────────────────────────────────────────────────

struct CloudDb {
    conn: Connection,
}

impl CloudDb {
    fn open() -> Result<Self> {
        let path = Config::data_dir()?.join("usage.db");
        let conn = Connection::open(path)?;
        conn.execute_batch(CLOUD_SCHEMA)?;
        Ok(Self { conn })
    }

    #[cfg(test)]
    fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(CLOUD_SCHEMA)?;
        Ok(Self { conn })
    }

    fn save(&self, r: &VmRecord) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO cloud_vms (provider,vm_id,name,region,size,ip,status,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                r.provider,
                r.vm_id,
                r.name,
                r.region,
                r.size,
                r.ip,
                r.status,
                r.created_at
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn list(&self) -> Result<Vec<VmRecord>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_ALL} ORDER BY created_at DESC"))?;
        let rows = stmt.query_map([], map_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn find(&self, query: &str) -> Result<Option<VmRecord>> {
        if let Ok(id) = query.parse::<i64>() {
            let r = self
                .conn
                .query_row(&format!("{SELECT_ALL} WHERE id=?1"), params![id], map_row)
                .optional()?;
            if r.is_some() {
                return Ok(r);
            }
        }
        self.conn
            .query_row(
                &format!("{SELECT_ALL} WHERE name=?1 OR vm_id=?1"),
                params![query],
                map_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn remove(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM cloud_vms WHERE id=?1", params![id])?;
        Ok(())
    }

    fn count(&self) -> Result<u32> {
        self.conn
            .query_row("SELECT COUNT(*) FROM cloud_vms", [], |r| r.get(0))
            .map_err(Into::into)
    }
}

fn map_row(row: &rusqlite::Row) -> rusqlite::Result<VmRecord> {
    Ok(VmRecord {
        id: row.get(0)?,
        provider: row.get(1)?,
        vm_id: row.get(2)?,
        name: row.get(3)?,
        region: row.get(4)?,
        size: row.get(5)?,
        ip: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
    })
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: CloudAction) -> Result<()> {
    match action {
        CloudAction::Up {
            provider,
            size,
            region,
            image,
            name,
        } => {
            up(
                &provider,
                size.as_deref(),
                region.as_deref(),
                image.as_deref(),
                name.as_deref(),
            )
            .await
        }
        CloudAction::Ls => ls(),
        CloudAction::Down { vm } => down(&vm).await,
        CloudAction::Ssh { vm } => ssh(&vm),
        CloudAction::Nl(_) => unreachable!("Nl handled in main"),
    }
}

// ── Up subcommand ─────────────────────────────────────────────────────────────

async fn up(
    provider_str: &str,
    size: Option<&str>,
    region: Option<&str>,
    image: Option<&str>,
    name: Option<&str>,
) -> Result<()> {
    let provider = Provider::from_str(provider_str)?;
    check_cli_available(&provider)?;
    let db = CloudDb::open()?;
    let cfg = Config::load()?;
    check_vm_limit(&db, &cfg)?;
    let spec = resolve_spec(&provider, size, region, image, name);
    let token = if provider.requires_token() {
        load_cloud_token(&provider)?
    } else {
        String::new()
    };
    println!("Provisioning {} VM `{}`...", provider_str, spec.name);
    let (vm_id, ip) = provision(&provider, &spec, &token).await?;
    let record = VmRecord {
        id: 0,
        provider: provider_str.to_string(),
        vm_id,
        name: spec.name,
        region: spec.region,
        size: spec.size,
        ip,
        status: "provisioning".into(),
        created_at: Local::now().format("%Y-%m-%d %H:%M").to_string(),
    };
    let local_id = db.save(&record)?;
    print_provisioned(local_id, &record, provider.default_ssh_user());
    Ok(())
}

fn resolve_spec(
    provider: &Provider,
    size: Option<&str>,
    region: Option<&str>,
    image: Option<&str>,
    name: Option<&str>,
) -> VmSpec {
    VmSpec {
        size: size.unwrap_or_else(|| provider.default_size()).to_string(),
        region: region
            .unwrap_or_else(|| provider.default_region())
            .to_string(),
        image: image
            .unwrap_or_else(|| provider.default_image())
            .to_string(),
        name: name.map(String::from).unwrap_or_else(generate_name),
    }
}

fn check_vm_limit(db: &CloudDb, cfg: &Config) -> Result<()> {
    let limit = cfg.usage.as_ref().map(|u| u.cloud_limit()).unwrap_or(3);
    let current = db.count()?;
    if current >= limit {
        bail!(
            "VM limit reached ({current}/{limit}). \
             Destroy one with `dclaw cloud down <name>` \
             or set cloud_limit in .devclawrc"
        );
    }
    Ok(())
}

fn load_cloud_token(provider: &Provider) -> Result<String> {
    let key_name = provider.keychain_name();
    creds::load(key_name).map_err(|_| {
        anyhow::anyhow!(
            "No API token for this provider. Run: dclaw config set-key --provider {key_name}"
        )
    })
}

fn print_provisioned(local_id: i64, r: &VmRecord, ssh_user: &str) {
    let ip_msg = r.ip.as_deref().unwrap_or("(assigned shortly)");
    println!("✓  Provisioned  : {} ({})", r.name, r.provider);
    println!("   VM ID        : {}", r.vm_id);
    println!("   IP address   : {ip_msg}");
    println!("   Size / Region: {} / {}", r.size, r.region);
    println!();
    println!("   Ready in ~30s. Connect with:");
    println!("   dclaw cloud ssh {local_id}   # or: ssh {ssh_user}@{ip_msg}");
    println!();
    println!("   To destroy: dclaw cloud down {local_id}");
}

// ── Ls subcommand ─────────────────────────────────────────────────────────────

fn ls() -> Result<()> {
    let db = CloudDb::open()?;
    let vms = db.list()?;
    if vms.is_empty() {
        println!("No VMs provisioned. Run `dclaw cloud up` to create one.");
        return Ok(());
    }
    println!(
        "{:<4} {:<12} {:<16} {:<14} {:<12} {:<17} {:<14} CREATED",
        "ID", "PROVIDER", "NAME", "SIZE", "REGION", "IP", "STATUS"
    );
    for v in &vms {
        let ip = v.ip.as_deref().unwrap_or("(provisioning)");
        println!(
            "{:<4} {:<12} {:<16} {:<14} {:<12} {:<17} {:<14} {}",
            v.id, v.provider, v.name, v.size, v.region, ip, v.status, v.created_at
        );
    }
    Ok(())
}

// ── Down subcommand ───────────────────────────────────────────────────────────

async fn down(vm: &str) -> Result<()> {
    let db = CloudDb::open()?;
    let record = db
        .find(vm)?
        .ok_or_else(|| anyhow::anyhow!("VM `{vm}` not found. Run `dclaw cloud ls`"))?;
    if !utils::confirm(&format!(
        "Permanently destroy VM `{}` ({})? This cannot be undone. [y/N] ",
        record.name, record.provider
    ))? {
        println!("Aborted.");
        return Ok(());
    }
    let provider = Provider::from_str(&record.provider)?;
    let token = if provider.requires_token() {
        load_cloud_token(&provider)?
    } else {
        String::new()
    };
    println!("Destroying {} `{}`...", record.provider, record.name);
    destroy(&provider, &record.vm_id, &record.region, &token).await?;
    db.remove(record.id)?;
    println!(
        "✓  Destroyed {} and removed from local records.",
        record.name
    );
    Ok(())
}

// ── Ssh subcommand ────────────────────────────────────────────────────────────

fn ssh(vm: &str) -> Result<()> {
    let db = CloudDb::open()?;
    let record = db
        .find(vm)?
        .ok_or_else(|| anyhow::anyhow!("VM `{vm}` not found. Run `dclaw cloud ls`"))?;
    let ip = record.ip.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "`{}` has no IP yet — still provisioning. Retry in ~30s.",
            record.name
        )
    })?;
    let user = Provider::from_str(&record.provider)
        .map(|p| p.default_ssh_user())
        .unwrap_or("root");
    println!("ssh {user}@{ip}");
    Ok(())
}

// ── Cloud API — DigitalOcean ──────────────────────────────────────────────────

async fn provision_do(spec: &VmSpec, token: &str) -> Result<(String, Option<String>)> {
    let body = serde_json::json!({
        "name": spec.name, "region": spec.region,
        "size": spec.size, "image":  spec.image,
    });
    let resp = reqwest::Client::new()
        .post("https://api.digitalocean.com/v2/droplets")
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("DigitalOcean error: {}", resp.text().await?);
    }
    let json: serde_json::Value = resp.json().await?;
    let vm_id = json["droplet"]["id"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("No droplet ID in response"))?
        .to_string();
    let ip = json["droplet"]["networks"]["v4"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|n| n["ip_address"].as_str())
        .map(String::from);
    Ok((vm_id, ip))
}

async fn destroy_do(vm_id: &str, token: &str) -> Result<()> {
    let resp = reqwest::Client::new()
        .delete(format!("https://api.digitalocean.com/v2/droplets/{vm_id}"))
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() && resp.status().as_u16() != 404 {
        bail!("DigitalOcean delete error: {}", resp.text().await?);
    }
    Ok(())
}

// ── Cloud API — Hetzner ───────────────────────────────────────────────────────

async fn provision_hetzner(spec: &VmSpec, token: &str) -> Result<(String, Option<String>)> {
    let body = serde_json::json!({
        "name": spec.name, "server_type": spec.size,
        "location": spec.region, "image": spec.image,
    });
    let resp = reqwest::Client::new()
        .post("https://api.hetzner.cloud/v1/servers")
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("Hetzner error: {}", resp.text().await?);
    }
    let json: serde_json::Value = resp.json().await?;
    let vm_id = json["server"]["id"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("No server ID in response"))?
        .to_string();
    let ip = json["server"]["public_net"]["ipv4"]["ip"]
        .as_str()
        .map(String::from);
    Ok((vm_id, ip))
}

async fn destroy_hetzner(vm_id: &str, token: &str) -> Result<()> {
    let resp = reqwest::Client::new()
        .delete(format!("https://api.hetzner.cloud/v1/servers/{vm_id}"))
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() && resp.status().as_u16() != 404 {
        bail!("Hetzner delete error: {}", resp.text().await?);
    }
    Ok(())
}

// ── Cloud CLI — AWS ───────────────────────────────────────────────────────────

fn provision_aws(spec: &VmSpec) -> Result<(String, Option<String>)> {
    // Use JSON format for --tag-specifications so that special characters in
    // spec.name cannot break the shorthand syntax parser in the AWS CLI.
    let tags = serde_json::json!([{
        "ResourceType": "instance",
        "Tags": [
            {"Key": "Name",      "Value": spec.name},
            {"Key": "CreatedBy", "Value": "dclaw"},
        ]
    }])
    .to_string();
    let out = std::process::Command::new("aws")
        .args([
            "ec2",
            "run-instances",
            "--image-id",
            &spec.image,
            "--instance-type",
            &spec.size,
            "--count",
            "1",
            "--region",
            &spec.region,
            "--tag-specifications",
            &tags,
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Cannot run `aws`: {e}"))?;
    if !out.status.success() {
        bail!(
            "aws ec2 run-instances failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let vm_id = json["Instances"][0]["InstanceId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No InstanceId in response"))?
        .to_string();
    let ip = json["Instances"][0]["PublicIpAddress"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok((vm_id, ip))
}

fn destroy_aws(vm_id: &str, region: &str) -> Result<()> {
    let out = std::process::Command::new("aws")
        .args([
            "ec2",
            "terminate-instances",
            "--instance-ids",
            vm_id,
            "--region",
            region,
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Cannot run `aws`: {e}"))?;
    if !out.status.success() {
        bail!(
            "aws ec2 terminate-instances failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

// ── Cloud CLI — Azure ─────────────────────────────────────────────────────────

fn provision_azure(spec: &VmSpec) -> Result<(String, Option<String>)> {
    ensure_azure_resource_group(&spec.region)?;
    let out = std::process::Command::new("az")
        .args([
            "vm",
            "create",
            "--resource-group",
            "dclaw-rg",
            "--name",
            &spec.name,
            "--image",
            &spec.image,
            "--size",
            &spec.size,
            "--location",
            &spec.region,
            "--admin-username",
            "azureuser",
            "--generate-ssh-keys",
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Cannot run `az`: {e}"))?;
    if !out.status.success() {
        bail!(
            "az vm create failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let ip = json["publicIpAddress"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok((format!("dclaw-rg/{}", spec.name), ip))
}

fn ensure_azure_resource_group(location: &str) -> Result<()> {
    let out = std::process::Command::new("az")
        .args([
            "group",
            "create",
            "--name",
            "dclaw-rg",
            "--location",
            location,
            "--output",
            "none",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Cannot run `az`: {e}"))?;
    if !out.status.success() {
        bail!(
            "Failed to create resource group: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

fn destroy_azure(vm_id: &str) -> Result<()> {
    let (rg, name) = vm_id
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("Invalid Azure VM ID format: {vm_id}"))?;
    let out = std::process::Command::new("az")
        .args([
            "vm",
            "delete",
            "--resource-group",
            rg,
            "--name",
            name,
            "--yes",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Cannot run `az`: {e}"))?;
    if !out.status.success() {
        bail!(
            "az vm delete failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

// ── Cloud CLI — GCP ───────────────────────────────────────────────────────────

fn provision_gcp(spec: &VmSpec) -> Result<(String, Option<String>)> {
    let (img_family, img_project) = parse_gcp_image(&spec.image);
    let out = std::process::Command::new("gcloud")
        .args([
            "compute",
            "instances",
            "create",
            &spec.name,
            "--machine-type",
            &spec.size,
            "--zone",
            &spec.region,
            "--image-family",
            &img_family,
            "--image-project",
            &img_project,
            "--labels",
            "created-by=dclaw",
            "--format",
            "json",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Cannot run `gcloud`: {e}"))?;
    if !out.status.success() {
        bail!(
            "gcloud instances create failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let vm_id = json[0]["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No instance name in GCP response"))?
        .to_string();
    let ip = json[0]["networkInterfaces"][0]["accessConfigs"][0]["natIP"]
        .as_str()
        .map(String::from);
    Ok((vm_id, ip))
}

fn destroy_gcp(vm_id: &str, zone: &str) -> Result<()> {
    let out = std::process::Command::new("gcloud")
        .args([
            "compute",
            "instances",
            "delete",
            vm_id,
            "--zone",
            zone,
            "--quiet",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("Cannot run `gcloud`: {e}"))?;
    if !out.status.success() {
        bail!(
            "gcloud instances delete failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Parses `"project/family"` or bare `"family"` (defaults project to `debian-cloud`).
pub fn parse_gcp_image(image: &str) -> (String, String) {
    match image.split_once('/') {
        Some((project, family)) => (family.to_string(), project.to_string()),
        None => (image.to_string(), "debian-cloud".to_string()),
    }
}

// ── CLI availability check ────────────────────────────────────────────────────

fn check_cli_available(provider: &Provider) -> Result<()> {
    let cli = match provider {
        Provider::Aws => "aws",
        Provider::Azure => "az",
        Provider::Gcp => "gcloud",
        _ => return Ok(()),
    };
    let ok = std::process::Command::new(cli)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        bail!("`{cli}` CLI not found. Install it and authenticate (e.g. `{cli} configure` / `{cli} auth login`).");
    }
    Ok(())
}

// ── Provider dispatch ─────────────────────────────────────────────────────────

async fn provision(
    provider: &Provider,
    spec: &VmSpec,
    token: &str,
) -> Result<(String, Option<String>)> {
    match provider {
        Provider::DigitalOcean => provision_do(spec, token).await,
        Provider::Hetzner => provision_hetzner(spec, token).await,
        Provider::Aws => provision_aws(spec),
        Provider::Azure => provision_azure(spec),
        Provider::Gcp => provision_gcp(spec),
    }
}

async fn destroy(provider: &Provider, vm_id: &str, region: &str, token: &str) -> Result<()> {
    match provider {
        Provider::DigitalOcean => destroy_do(vm_id, token).await,
        Provider::Hetzner => destroy_hetzner(vm_id, token).await,
        Provider::Aws => destroy_aws(vm_id, region),
        Provider::Azure => destroy_azure(vm_id),
        Provider::Gcp => destroy_gcp(vm_id, region),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn generate_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("claw-{:06x}", nanos & 0x00ff_ffff)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UsageLimits;

    fn sample_record(name: &str, vm_id: &str) -> VmRecord {
        VmRecord {
            id: 0,
            provider: "do".into(),
            vm_id: vm_id.into(),
            name: name.into(),
            region: "nyc3".into(),
            size: "s-1vcpu-1gb".into(),
            ip: Some("1.2.3.4".into()),
            status: "active".into(),
            created_at: "2024-01-01 12:00".into(),
        }
    }

    fn cfg_with_cloud_limit(limit: u32) -> Config {
        Config {
            stack: None,
            doctor: None,
            git: None,
            workflows: None,
            usage: Some(UsageLimits {
                monthly_limit: None,
                doctor_limit: None,
                git_limit: None,
                env_limit: None,
                forensic_limit: None,
                mock_limit: None,
                cloud_limit: Some(limit),
                warn_at_percent: None,
                reset_day: None,
            }),
        }
    }

    // --- Provider::from_str ---

    #[test]
    fn provider_from_str_do_aliases() {
        assert!(Provider::from_str("do").is_ok());
        assert!(Provider::from_str("digitalocean").is_ok());
    }

    #[test]
    fn provider_from_str_hetzner_aliases() {
        assert!(Provider::from_str("hetzner").is_ok());
        assert!(Provider::from_str("hz").is_ok());
    }

    #[test]
    fn provider_from_str_aws_aliases() {
        assert!(Provider::from_str("aws").is_ok());
        assert!(Provider::from_str("amazon").is_ok());
    }

    #[test]
    fn provider_from_str_azure_aliases() {
        assert!(Provider::from_str("azure").is_ok());
        assert!(Provider::from_str("az").is_ok());
    }

    #[test]
    fn provider_from_str_gcp_aliases() {
        assert!(Provider::from_str("gcp").is_ok());
        assert!(Provider::from_str("google").is_ok());
    }

    #[test]
    fn provider_from_str_invalid() {
        assert!(Provider::from_str("heroku").is_err());
        assert!(Provider::from_str("").is_err());
    }

    // --- Provider defaults ---

    #[test]
    fn provider_default_sizes() {
        assert_eq!(Provider::DigitalOcean.default_size(), "s-1vcpu-1gb");
        assert_eq!(Provider::Aws.default_size(), "t3.micro");
        assert_eq!(Provider::Azure.default_size(), "Standard_B1s");
        assert_eq!(Provider::Gcp.default_size(), "e2-micro");
        assert_eq!(Provider::Hetzner.default_size(), "cx11");
    }

    #[test]
    fn provider_default_regions() {
        assert_eq!(Provider::Aws.default_region(), "us-east-1");
        assert_eq!(Provider::Azure.default_region(), "eastus");
        assert_eq!(Provider::Gcp.default_region(), "us-central1-a");
    }

    // --- SSH users ---

    #[test]
    fn provider_ssh_users() {
        assert_eq!(Provider::DigitalOcean.default_ssh_user(), "root");
        assert_eq!(Provider::Hetzner.default_ssh_user(), "root");
        assert_eq!(Provider::Aws.default_ssh_user(), "ec2-user");
        assert_eq!(Provider::Azure.default_ssh_user(), "azureuser");
        assert_eq!(Provider::Gcp.default_ssh_user(), "user");
    }

    // --- requires_token ---

    #[test]
    fn provider_requires_token_api_providers() {
        assert!(Provider::DigitalOcean.requires_token());
        assert!(Provider::Hetzner.requires_token());
    }

    #[test]
    fn provider_requires_token_cli_providers() {
        assert!(!Provider::Aws.requires_token());
        assert!(!Provider::Azure.requires_token());
        assert!(!Provider::Gcp.requires_token());
    }

    // --- parse_gcp_image ---

    #[test]
    fn parse_gcp_image_with_project_slash_family() {
        let (family, project) = parse_gcp_image("ubuntu-os-cloud/ubuntu-2204-lts");
        assert_eq!(family, "ubuntu-2204-lts");
        assert_eq!(project, "ubuntu-os-cloud");
    }

    #[test]
    fn parse_gcp_image_bare_uses_debian_cloud() {
        let (family, project) = parse_gcp_image("debian-11");
        assert_eq!(family, "debian-11");
        assert_eq!(project, "debian-cloud");
    }

    // --- resolve_spec ---

    #[test]
    fn resolve_spec_do_defaults() {
        let spec = resolve_spec(&Provider::DigitalOcean, None, None, None, Some("my-vm"));
        assert_eq!(spec.size, "s-1vcpu-1gb");
        assert_eq!(spec.region, "nyc3");
        assert_eq!(spec.image, "ubuntu-22-04-x64");
    }

    #[test]
    fn resolve_spec_aws_defaults() {
        let spec = resolve_spec(&Provider::Aws, None, None, None, Some("my-vm"));
        assert_eq!(spec.size, "t3.micro");
        assert_eq!(spec.region, "us-east-1");
    }

    #[test]
    fn resolve_spec_custom_overrides_defaults() {
        let spec = resolve_spec(
            &Provider::Aws,
            Some("m5.large"),
            Some("eu-west-1"),
            None,
            Some("x"),
        );
        assert_eq!(spec.size, "m5.large");
        assert_eq!(spec.region, "eu-west-1");
    }

    // --- generate_name ---

    #[test]
    fn generate_name_has_claw_prefix() {
        assert!(generate_name().starts_with("claw-"));
    }

    #[test]
    fn generate_name_correct_length() {
        assert_eq!(generate_name().len(), 11);
    }

    // --- CloudDb ---

    #[test]
    fn cloud_db_empty_list() {
        let db = CloudDb::open_memory().unwrap();
        assert!(db.list().unwrap().is_empty());
    }

    #[test]
    fn cloud_db_save_and_list() {
        let db = CloudDb::open_memory().unwrap();
        db.save(&sample_record("test-vm", "111")).unwrap();
        let vms = db.list().unwrap();
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].name, "test-vm");
    }

    #[test]
    fn cloud_db_find_by_local_id() {
        let db = CloudDb::open_memory().unwrap();
        let id = db.save(&sample_record("my-vm", "999")).unwrap();
        assert_eq!(db.find(&id.to_string()).unwrap().unwrap().name, "my-vm");
    }

    #[test]
    fn cloud_db_find_by_name() {
        let db = CloudDb::open_memory().unwrap();
        db.save(&sample_record("named-vm", "888")).unwrap();
        assert_eq!(db.find("named-vm").unwrap().unwrap().vm_id, "888");
    }

    #[test]
    fn cloud_db_find_by_vm_id() {
        let db = CloudDb::open_memory().unwrap();
        db.save(&sample_record("vm-x", "777")).unwrap();
        assert_eq!(db.find("777").unwrap().unwrap().name, "vm-x");
    }

    #[test]
    fn cloud_db_find_missing_returns_none() {
        assert!(CloudDb::open_memory()
            .unwrap()
            .find("none")
            .unwrap()
            .is_none());
    }

    #[test]
    fn cloud_db_remove_deletes_record() {
        let db = CloudDb::open_memory().unwrap();
        let id = db.save(&sample_record("del-vm", "666")).unwrap();
        db.remove(id).unwrap();
        assert!(db.find("del-vm").unwrap().is_none());
    }

    #[test]
    fn cloud_db_count_tracks_saves() {
        let db = CloudDb::open_memory().unwrap();
        assert_eq!(db.count().unwrap(), 0);
        db.save(&sample_record("vm1", "aaa")).unwrap();
        db.save(&sample_record("vm2", "bbb")).unwrap();
        assert_eq!(db.count().unwrap(), 2);
    }

    // --- check_vm_limit ---

    #[test]
    fn check_vm_limit_allows_under_limit() {
        let db = CloudDb::open_memory().unwrap();
        assert!(check_vm_limit(&db, &cfg_with_cloud_limit(3)).is_ok());
    }

    #[test]
    fn check_vm_limit_blocks_at_limit() {
        let db = CloudDb::open_memory().unwrap();
        for i in 0..3 {
            db.save(&sample_record(&format!("vm{i}"), &i.to_string()))
                .unwrap();
        }
        assert!(check_vm_limit(&db, &cfg_with_cloud_limit(3)).is_err());
    }
}
