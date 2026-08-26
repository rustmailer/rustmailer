// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use clap::{builder::ValueParser, Parser, ValueEnum};
use std::{collections::HashSet, env, fmt, path::PathBuf, sync::LazyLock};
use url::Url;

#[cfg(not(test))]
pub static SETTINGS: LazyLock<Settings> = LazyLock::new(Settings::parse);

#[cfg(test)]
pub static SETTINGS: LazyLock<Settings> = LazyLock::new(Settings::new_for_test);

#[derive(Debug, Parser)]
#[clap(
    name = "rustmailer",
    about = "A system for managing multiple email accounts via REST,
    simplifying integration of email services into applications without dealing with complex IMAP protocols.",
    version = env!("CARGO_PKG_VERSION")
)]
pub struct Settings {
    /// rustmailer log level (default: "info")
    #[clap(
        long,
        default_value = "info",
        env,
        help = "Set the log level for rustmailer"
    )]
    pub rustmailer_log_level: String,

    /// rustmailer HTTP port (default: 15630)
    #[clap(
        long,
        default_value = "15630",
        env,
        help = "Set the HTTP port for rustmailer"
    )]
    pub rustmailer_http_port: i32,

    /// The IP address that the node binds to, in IPv4 format (e.g., 192.168.1.1).
    /// Required when running in cluster mode (`rustmailer_cluster_mode = true`), unless this node is the master.
    #[clap(
        long,
        env,
        default_value = "0.0.0.0",
        help = "The IP address that the node binds to, in IPv4 format (e.g., 192.168.1.1). Required in cluster mode.",
        value_parser = ValueParser::new(|s: &str| {
            // Ensure the input is a valid IPv4 address
            if s.parse::<std::net::Ipv4Addr>().is_err() {
                return Err("The bind IP address must be a valid IPv4 address.".to_string());
            }

            // If the address is valid, return it
            Ok(s.to_string())
        })
    )]
    pub rustmailer_bind_ip: Option<String>,

    /// rustmailer grpc port (default: 16630)
    #[clap(
        long,
        default_value = "16630",
        env,
        help = "Set the Grpc port for rustmailer"
    )]
    pub rustmailer_grpc_port: i32,

    /// RustMail public URL (default: "http://localhost:15630")
    #[clap(
        long,
        default_value = "http://localhost:15630",
        env,
        help = "Set the public URL for rustmailer"
    )]
    pub rustmailer_public_url: String,

    /// Base URL for email tracking (e.g., "https://track.example.com")
    #[clap(
        long,
        default_value = "http://localhost:15630/email-track",
        help = "Set the base URL for email tracking (e.g., for open/click tracking)"
    )]
    pub rustmailer_email_tracking_url: String,

    /// CORS allowed origins (default: "*")
    #[clap(
        long,
        default_value = "http://localhost:5173, http://localhost:15630, http://192.168.3.2:15630, *",
        env,
        help = "Set the allowed CORS origins (comma-separated list, e.g., \"https://example.com, https://another.com\")",
        value_parser = ValueParser::new(|s: &str| -> Result<HashSet<String>, String> {
            let set: HashSet<String> = s.split(',')
                .map(|origin| origin.trim().to_string())
                .filter(|origin| !origin.is_empty())
                .collect();
            Ok(set)
        })
    )]
    pub rustmailer_cors_origins: Option<HashSet<String>>,

    /// CORS max age in seconds (default: 86400)
    #[clap(
        long,
        default_value = "86400",
        env,
        help = "Set the CORS max age in seconds"
    )]
    pub rustmailer_cors_max_age: i32,

    #[clap(
        long,
        default_value = "20",
        env,
        help = "Set the number of workers for sending mail tasks"
    )]
    pub rustmailer_send_mail_workers: usize,

    #[clap(
        long,
        default_value = "100000",  // Default to 100,000 characters
        help = "Maximum length (in characters) for email text content retrieval",
        value_parser = clap::value_parser!(u32).range(50..)
    )]
    pub rustmailer_max_email_content_length: u32,

    #[clap(
        long,
        default_value = "20",
        env,
        help = "Set the number of workers for event hook tasks"
    )]
    pub rustmailer_event_hook_workers: usize,

    /// Enable ANSI logs (default: false)
    #[clap(long, default_value = "true", env, help = "Enable ANSI formatted logs")]
    pub rustmailer_ansi_logs: bool,

    /// Enable log file output (default: false)
    /// If false, logs will be printed to stdout
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enable log file output (otherwise logs go to stdout)"
    )]
    pub rustmailer_log_to_file: bool,

    /// Enable JSON logs (default: false)
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enable JSON formatted logs"
    )]
    pub rustmailer_json_logs: bool,

    /// Maximum number of log files (default: 5)
    #[clap(
        long,
        default_value = "5",
        env,
        help = "Set the maximum number of server log files"
    )]
    pub rustmailer_max_server_log_files: usize,

    /// rustmailer encryption password
    #[clap(
        long,
        default_value = "change-this-default-password-now",
        env,
        help = "Set the encryption password for rustmailer. ⚠️ Change this default in production!"
    )]
    pub rustmailer_encrypt_password: String,

    #[clap(
        long,
        env,
        help = "Set the file path for rustmailer database",
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);
            if !path.is_absolute() {
                return Err("Path must be an absolute directory path".to_string());
            }
            if !path.exists() {
                return Err(format!("Path {:?} does not exist", path));
            }
            if !path.is_dir() {
                return Err(format!("Path {:?} is not a directory", path));
            }
            Ok(s.to_string())
        })
    )]
    pub rustmailer_root_dir: String,

    #[clap(
        long,
        env,
        default_value = "134217728",
        help = "Set the cache size for rustmailer metadata database in bytes"
    )]
    pub rustmailer_metadata_cache_size: Option<usize>,

    #[clap(
        long,
        env,
        default_value = "67108864",
        help = "Set the cache size for task queue database in bytes"
    )]
    pub rustmailer_task_queue_cache_size: Option<usize>,

    #[clap(
        long,
        env,
        default_value = "1073741824",
        help = "Set the cache size for envelope database in bytes"
    )]
    pub rustmailer_envelope_cache_size: Option<usize>,

    /// Abort the process when the tokio runtime stops producing heartbeats.
    ///
    /// A watchdog thread exits with a non-zero code when no runtime heartbeat is
    /// observed for this many seconds, letting orchestrators (Docker restart
    /// policies, supervisors) recover the process from a wedged runtime.
    /// Set to 0 to disable the watchdog.
    #[clap(
        long,
        env,
        default_value = "60",
        help = "Abort the process if the runtime heartbeat is stale for this many seconds (0 disables the watchdog)"
    )]
    pub rustmailer_watchdog_timeout_secs: u64,

    /// Enables or disables the access token mechanism for HTTP endpoints.
    ///
    /// When set to `true`, HTTP requests will be subject to access token validation.
    /// If the `Authorization` header is missing or the token is invalid, the service will return a 401 Unauthorized response.
    /// When set to `false`, access token validation will be skipped.
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enables or disables the access token mechanism for HTTP endpoints."
    )]
    pub rustmailer_enable_access_token: bool,

    /// Enables or disables HTTPS for REST API endpoints.
    ///
    /// When set to `true`, the REST API will use HTTPS with a valid SSL/TLS certificate for secure communication.
    /// If no valid certificate is configured or HTTPS cannot be established, the service will fail to start.
    /// When set to `false`, the REST API will use plain HTTP without encryption.
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enables or disables HTTPS for REST API endpoints."
    )]
    pub rustmailer_enable_rest_https: bool,

    /// Enables or disables HTTPS for gRPC endpoints.
    ///
    /// When set to `true`, the gRPC service will use HTTPS (HTTP/2 over TLS) with a valid SSL/TLS certificate for secure communication.
    /// If no valid certificate is configured or HTTPS cannot be established, the service will fail to start.
    /// When set to `false`, the gRPC service will use plain HTTP/2 without encryption.
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enables or disables HTTPS for gRPC endpoints."
    )]
    pub rustmailer_enable_grpc_https: bool,

    /// Enables or disables email tracking globally.
    #[clap(
        long,
        default_value = "true",
        env,
        help = "Enables or disables email tracking globally."
    )]
    pub rustmailer_email_tracking_enabled: bool,

    /// Enable gRPC server (default: true)
    #[clap(long, default_value = "true", env, help = "Enable the gRPC server")]
    pub rustmailer_grpc_enabled: bool,

    #[clap(
        long,
        default_value = "gzip",
        help = "Specify compression algorithm for gRPC responses (options: none, gzip, brotli, zstd, deflate; default: none)"
    )]
    pub rustmailer_grpc_compression: CompressionAlgorithm,

    #[clap(
        long,
        default_value = "true",
        env,
        help = "Enable compression for the open api server"
    )]
    pub rustmailer_http_compression_enabled: bool,

    #[clap(
        long,
        default_value = "72",
        env,
        help = "The interval (in hours) between task database cleanup operations. Tasks older than this duration will be removed during cleanup.",
        value_parser = ValueParser::new(|s: &str| {
            // Parse the string to a u64
            let value = s.parse::<u64>().map_err(|_| {
                format!(
                    "Invalid value: {}. Please provide a valid number of hours.",
                    s
                )
            })?;

            // Set min and max limits
            if value < 1 {
                return Err("Cleanup interval must be at least 1 hour.".to_string());
            }
            if value > 720 {
                return Err("Cleanup interval must be at most 720 hours (30 days).".to_string());
            }

            Ok(value)
        })
    )]
    pub rustmailer_cleanup_interval_hours: u64,

    #[clap(
        long,
        help = "Set the directory for storing backups of meta.db and tasks.db (must exist and have read/write permissions)",
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);
            // Check if the directory exists
            if !path.exists() {
                return Err(format!("Backup directory does not exist: {:?}", path));
            }

            // Check if the path is a directory
            if !path.is_dir() {
                return Err(format!("Backup path is not a directory: {:?}", path));
            }

            // Check read permission by attempting to read the directory
            if std::fs::read_dir(&path).is_err() {
                return Err(format!("Backup directory lacks read permission: {:?}", path));
            }

            // Check write permission by attempting to create a temporary file
            let temp_file = path.join(".rustmailer_test_write");
            if std::fs::write(&temp_file, "").is_err() {
                return Err(format!("Backup directory lacks write permission: {:?}", path));
            }
            // Clean up the test file
            let _ = std::fs::remove_file(&temp_file);

            Ok(path)
        })
    )]
    pub rustmailer_backup_dir: Option<PathBuf>,

    #[clap(
        long,
        default_value = "10",
        help = "Set the maximum number of backups to retain for each database file (default: 10)",
        value_parser = ValueParser::new(|s: &str| {
            let value: usize = s.parse().map_err(|_| format!("Invalid number for max_backups: {}", s))?;
            if value < 1 {
                return Err(format!("Maximum number of backups must be at least 1, got {}", value));
            }
            Ok(value)
        })
    )]
    pub rustmailer_max_backups: usize,

    #[clap(
        long,
        env,
        default_value = "false",
        help = "Keep metadata in memory and periodically persist it to disk"
    )]
    pub rustmailer_metadata_memory_mode_enabled: bool,

    #[clap(
        long,
        env,
        default_value = "true",
        help = "Periodically send NOOP commands to keep IMAP connections alive"
    )]
    pub rustmailer_imap_keepalive_enabled: bool,

    #[clap(
        long,
        env,
        default_value = "900",
        help = "Interval in seconds to persist in-memory metadata to disk (minimum: 60)",
        value_parser = clap::value_parser!(u64).range(60..)
    )]
    pub rustmailer_metadata_snapshot_interval_secs: u64,

    #[clap(
        long,
        env,
        help = "URL to redirect users to after successful OAuth2 authorization",
        value_parser = ValueParser::new(|s: &str| -> Result<String, String> {
            Url::parse(s).map_err(|_| format!("Invalid URL for oauth2_success_redirect: {}", s))?;
            Ok(s.to_string())
        })
    )]
    pub rustmailer_oauth2_success_redirect: Option<String>,

    #[clap(
        long,
        env,
        help = "Maximum number of concurrent email sync tasks (default: number of CPU cores x 2)",
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub rustmailer_sync_concurrency: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum CompressionAlgorithm {
    #[clap(name = "none")]
    None,
    #[clap(name = "gzip")]
    Gzip,
    #[clap(name = "brotli")]
    Brotli,
    #[clap(name = "zstd")]
    Zstd,
    #[clap(name = "deflate")]
    Deflate,
}

impl fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompressionAlgorithm::None => write!(f, "none"),
            CompressionAlgorithm::Gzip => write!(f, "gzip"),
            CompressionAlgorithm::Brotli => write!(f, "brotli"),
            CompressionAlgorithm::Zstd => write!(f, "zstd"),
            CompressionAlgorithm::Deflate => write!(f, "deflate"),
        }
    }
}

impl Settings {
    #[cfg(test)]
    fn new_for_test() -> Self {
        Self {
            rustmailer_log_level: "info".to_string(),
            rustmailer_http_port: 15630,
            rustmailer_grpc_port: 16630,
            rustmailer_public_url: "http://localhost:15630".to_string(),
            rustmailer_ansi_logs: false,
            rustmailer_json_logs: false,
            rustmailer_log_to_file: false,
            rustmailer_imap_keepalive_enabled: false,
            rustmailer_max_server_log_files: 5,
            rustmailer_send_mail_workers: 10,
            rustmailer_encrypt_password: "change-this-default-password-now".into(),
            rustmailer_root_dir: if cfg!(windows) {
                "D:\\rustmailer_data".into()
            } else {
                "/sourcecode/rustmailer/rustmailer_data".into()
            },
            rustmailer_metadata_cache_size: None,
            rustmailer_task_queue_cache_size: None,
            rustmailer_envelope_cache_size: None,
            rustmailer_enable_access_token: false,
            rustmailer_email_tracking_enabled: false,
            rustmailer_bind_ip: Default::default(),
            rustmailer_cors_origins: Default::default(),
            rustmailer_cors_max_age: 86400,
            rustmailer_grpc_enabled: true,
            rustmailer_enable_rest_https: false,
            rustmailer_enable_grpc_https: false,
            rustmailer_grpc_compression: CompressionAlgorithm::None,
            rustmailer_http_compression_enabled: true,
            rustmailer_event_hook_workers: 10,
            rustmailer_max_email_content_length: 10000,
            rustmailer_cleanup_interval_hours: 72,
            rustmailer_backup_dir: None,
            rustmailer_max_backups: 10,
            rustmailer_email_tracking_url: "http://localhost:15630/email-track".to_string(),
            rustmailer_metadata_memory_mode_enabled: false,
            rustmailer_metadata_snapshot_interval_secs: 900,
            rustmailer_oauth2_success_redirect: None,
            rustmailer_sync_concurrency: Some(5),
            rustmailer_watchdog_timeout_secs: 0,
        }
    }
}
