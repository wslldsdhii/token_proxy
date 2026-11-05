use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "token-proxy")]
#[command(about = "Token Proxy CLI (no Tauri required)")]
struct Cli {
    /// 配置文件路径；默认使用 ./config.jsonc
    #[arg(long, default_value = "./config.jsonc")]
    config: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 启动代理服务（读取配置并监听 host:port）
    Serve,
    /// 启动 Agent Console 子节点
    AgentNode(AgentNodeCommand),
    /// 配置相关命令
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Parser)]
struct AgentNodeCommand {
    /// Agent Console 公网地址，例如 https://agent.example.com
    #[arg(long)]
    server_url: String,

    /// 用户在 Agent Console 里创建的 node API key
    #[arg(long)]
    api_key: String,

    /// 上报给 Agent Console 的节点主机名；默认读取系统环境变量
    #[arg(long)]
    hostname: Option<String>,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// 打印当前实际使用的配置文件路径
    Path,
    /// 在目标路径创建默认配置（若已存在则报错）
    Init,
    // ══════════ HEADER-INJECTION PATCH 8 (cli command) START ══════════
    /// 渠道级请求头注入配置（会话标记 → 请求头）
    HeaderInjection {
        #[command(subcommand)]
        command: HeaderInjectionCommand,
    },
    // ══════════ HEADER-INJECTION PATCH 8 (cli command) END ══════════
}

// ══════════ HEADER-INJECTION PATCH 8 (cli subcommands) START ══════════
#[derive(Subcommand)]
enum HeaderInjectionCommand {
    /// 列出所有渠道的头注入配置（含孤儿记录）
    List,
    /// 启用（或更新）某渠道的头注入
    Enable {
        /// 渠道 id（upstream id）
        #[arg(long)]
        channel: String,
        /// 目标 Header；缺省 X-Session-ID，可选 X-Opencode-Session
        #[arg(long)]
        header: Option<String>,
        /// 开启无标记/无会话头时的注入回退（保留已有回退设置）
        #[arg(long)]
        fallback: bool,
    },
    /// 关闭某渠道的头注入（保留记录）
    Disable {
        /// 渠道 id（upstream id）
        #[arg(long)]
        channel: String,
    },
}
// ══════════ HEADER-INJECTION PATCH 8 (cli subcommands) END ══════════

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli).await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let paths = token_proxy_account_store::paths::TokenProxyPaths::from_config_path(&cli.config)?;
    match cli.command {
        Command::Config { command } => match command {
            ConfigCommand::Path => {
                println!("{}", paths.config_file().display());
                Ok(())
            }
            ConfigCommand::Init => {
                token_proxy_config::init_default_config(&paths).await?;
                println!("created {}", paths.config_file().display());
                Ok(())
            }
            // ══════════ HEADER-INJECTION PATCH 9 (cli dispatch) START ══════════
            ConfigCommand::HeaderInjection { command } => {
                run_header_injection(paths, command).await
            } // ══════════ HEADER-INJECTION PATCH 9 (cli dispatch) END ══════════
        },
        Command::Serve => serve(paths).await,
        Command::AgentNode(command) => run_agent_node(command).await,
    }
}

// ══════════ HEADER-INJECTION PATCH 10 (cli impl) START ══════════
async fn run_header_injection(
    paths: token_proxy_account_store::paths::TokenProxyPaths,
    command: HeaderInjectionCommand,
) -> Result<(), String> {
    let mut config = token_proxy_config::read_config(&paths).await?.config;
    if matches!(command, HeaderInjectionCommand::List) {
        print_header_injection_list(&config);
        return Ok(());
    }
    let (changed, message) = mutate_header_injection(&mut config, command)?;
    if changed {
        // write_config 内部会走完整 build_runtime_config 校验；非法 Header 在更早处已被拒绝。
        token_proxy_config::write_config(&paths, config).await?;
    }
    println!("{message}");
    Ok(())
}

fn mutate_header_injection(
    config: &mut token_proxy_config::ProxyConfigFile,
    command: HeaderInjectionCommand,
) -> Result<(bool, String), String> {
    use token_proxy_config::header_injection::RULE_ID_SESSION;

    match command {
        HeaderInjectionCommand::List => Ok((false, String::new())),
        HeaderInjectionCommand::Enable {
            channel,
            header,
            fallback,
        } => {
            // 先校验显式目标 Header，避免失败时在内存里留下半成品段（写盘虽已被 ? 阻断）。
            if let Some(value) = header.as_deref() {
                token_proxy_config::header_injection::normalize_target_header(value)?;
            }
            let section = config.header_injection.get_or_insert_with(Default::default);
            let fallback = fallback.then_some(true);
            section.enable_channel(&channel, header.as_deref(), fallback)?;
            let target = section
                .channels
                .get(channel.trim())
                .and_then(|value| {
                    value
                        .rules
                        .iter()
                        .find(|rule| rule.id.trim() == RULE_ID_SESSION)
                })
                .map(|rule| rule.target_header.as_str())
                .unwrap_or("-");
            Ok((
                true,
                format!(
                    "enabled header injection for channel {} -> {target}",
                    channel.trim()
                ),
            ))
        }
        HeaderInjectionCommand::Disable { channel } => {
            let Some(section) = config.header_injection.as_mut() else {
                return Ok((
                    false,
                    format!(
                        "channel {} has no header injection record; nothing to do",
                        channel.trim()
                    ),
                ));
            };
            if section.disable_channel(&channel) {
                Ok((
                    true,
                    format!(
                        "disabled header injection for channel {} (record kept)",
                        channel.trim()
                    ),
                ))
            } else {
                Ok((
                    false,
                    format!(
                        "channel {} has no header injection record; nothing to do",
                        channel.trim()
                    ),
                ))
            }
        }
    }
}

fn print_header_injection_list(config: &token_proxy_config::ProxyConfigFile) {
    let Some(section) = config.header_injection.as_ref() else {
        println!("no header_injection section configured");
        return;
    };
    if section.channels.is_empty() {
        println!("no channels configured");
        return;
    }
    println!(
        "{:<28} {:<8} {:<9} {:<24} {:<24}",
        "CHANNEL", "ENABLED", "FALLBACK", "SOURCE", "TARGET_HEADER"
    );
    for row in header_injection_rows(config, section) {
        println!("{row}");
    }
}

fn header_injection_rows(
    config: &token_proxy_config::ProxyConfigFile,
    section: &token_proxy_config::header_injection::HeaderInjectionSection,
) -> Vec<String> {
    use token_proxy_config::header_injection::RULE_ID_SESSION;

    section
        .channels
        .iter()
        .map(|(id, channel)| {
            let rule = channel
                .rules
                .iter()
                .find(|rule| rule.id.trim() == RULE_ID_SESSION);
            let source = rule.map(|rule| rule.source.as_str()).unwrap_or("-");
            let target = rule.map(|rule| rule.target_header.as_str()).unwrap_or("-");
            let orphan = !config
                .upstreams
                .iter()
                .any(|upstream| upstream.id.trim() == id.trim());
            format!(
                "{:<28} {:<8} {:<9} {:<24} {:<24}{}",
                id,
                channel.enabled,
                channel.fallback_enabled,
                source,
                target,
                if orphan { "  (channel not found)" } else { "" }
            )
        })
        .collect()
}
// ══════════ HEADER-INJECTION PATCH 10 (cli impl) END ══════════

async fn run_agent_node(command: AgentNodeCommand) -> Result<(), String> {
    let config = token_proxy_agent_node::AgentNodeConfig {
        server_url: command.server_url,
        api_key: command.api_key,
        hostname: command.hostname.or_else(default_hostname),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    println!("agent node connecting to {}", config.server_url);
    let mut client = token_proxy_agent_node::AgentNodeClient::new(config);
    tokio::select! {
        result = client.run_with_reconnect() => result,
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|err| format!("Failed to listen for Ctrl+C: {err}"))?;
            println!("agent node stopped");
            Ok(())
        }
    }
}

fn default_hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn serve(paths: token_proxy_account_store::paths::TokenProxyPaths) -> Result<(), String> {
    // 应用 composition root 统一 CLI/Tauri 的账户、统计和代理依赖。
    let logging =
        token_proxy_app::logging::LoggingState::init(token_proxy_app::logging::LogLevel::Silent);
    let app = token_proxy_app::app::TokenProxyApp::open(paths, logging, None)?;
    let paths = app.paths();
    let app_proxy = app.app_proxy();
    let config_file = token_proxy_config::read_config(&paths).await?.config;
    let proxy_url = token_proxy_config::app_proxy_url_from_config(&config_file)?;
    token_proxy_account_store::app_proxy::set(&app_proxy, proxy_url.clone()).await;

    // 价格目录刷新独立于代理启动；失败时继续使用 SQLite 缓存或内置快照。
    let pricing_app = app.clone();
    tokio::spawn(async move {
        let result = pricing_app
            .refresh_model_pricing_catalog(proxy_url.as_deref())
            .await
            .map(|_| ());
        if let Err(err) = result {
            eprintln!("model pricing catalog refresh failed: {err}");
        }
    });

    let status = app.start_proxy().await?;
    if let Some(addr) = status.addr.as_deref() {
        println!("proxy running on {addr}");
    } else {
        println!("proxy started");
    }

    // 4) 等待退出信号，优雅停机
    tokio::signal::ctrl_c()
        .await
        .map_err(|err| format!("Failed to listen for Ctrl+C: {err}"))?;
    let _ = app.stop_proxy().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agent_node_command() {
        let cli = Cli::try_parse_from([
            "token-proxy",
            "agent-node",
            "--server-url",
            "https://agent.example.com",
            "--api-key",
            "acn_secret",
            "--hostname",
            "desk-1",
        ])
        .expect("parse agent node command");

        match cli.command {
            Command::AgentNode(command) => {
                assert_eq!(command.server_url, "https://agent.example.com");
                assert_eq!(command.api_key, "acn_secret");
                assert_eq!(command.hostname.as_deref(), Some("desk-1"));
            }
            _ => panic!("expected agent-node command"),
        }
    }

    // ══════════ HEADER-INJECTION PATCH 11 (cli tests) START ══════════
    fn parse_config_command(args: &[&str]) -> ConfigCommand {
        let cli = Cli::try_parse_from(args).expect("parse config command");
        match cli.command {
            Command::Config { command } => command,
            _ => panic!("expected config command"),
        }
    }

    #[test]
    fn parses_header_injection_enable() {
        match parse_config_command(&[
            "token-proxy",
            "config",
            "header-injection",
            "enable",
            "--channel",
            "trae",
            "--header",
            "X-Opencode-Session",
            "--fallback",
        ]) {
            ConfigCommand::HeaderInjection {
                command: HeaderInjectionCommand::Enable {
                    channel,
                    header,
                    fallback,
                },
            } => {
                assert_eq!(channel, "trae");
                assert_eq!(header.as_deref(), Some("X-Opencode-Session"));
                assert!(fallback);
            }
            _ => panic!("expected header-injection enable"),
        }
    }

    #[test]
    fn parses_header_injection_enable_without_fallback() {
        match parse_config_command(&[
            "token-proxy",
            "config",
            "header-injection",
            "enable",
            "--channel",
            "trae",
        ]) {
            ConfigCommand::HeaderInjection {
                command: HeaderInjectionCommand::Enable { fallback, .. },
            } => assert!(!fallback),
            _ => panic!("expected header-injection enable"),
        }
    }

    #[test]
    fn parses_header_injection_disable_and_list() {
        match parse_config_command(&[
            "token-proxy",
            "config",
            "header-injection",
            "disable",
            "--channel",
            "trae",
        ]) {
            ConfigCommand::HeaderInjection {
                command: HeaderInjectionCommand::Disable { channel },
            } => assert_eq!(channel, "trae"),
            _ => panic!("expected header-injection disable"),
        }

        match parse_config_command(&["token-proxy", "config", "header-injection", "list"]) {
            ConfigCommand::HeaderInjection {
                command: HeaderInjectionCommand::List,
            } => {}
            _ => panic!("expected header-injection list"),
        }
    }

    #[test]
    fn header_injection_enable_creates_and_updates_record() {
        let mut config = token_proxy_config::ProxyConfigFile::default();

        let (changed, message) = mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Enable {
                channel: "trae".to_string(),
                header: None,
                fallback: false,
            },
        )
        .expect("enable");
        assert!(changed);
        assert!(message.contains("X-Session-ID"));
        let channel = config
            .header_injection
            .as_ref()
            .expect("section")
            .channels
            .get("trae")
            .expect("channel");
        assert!(channel.enabled);
        assert_eq!(channel.rules[0].target_header, "X-Session-ID");

        let (changed, _) = mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Enable {
                channel: "trae".to_string(),
                header: Some("X-Opencode-Session".to_string()),
                fallback: false,
            },
        )
        .expect("update");
        assert!(changed);
        let channel = config
            .header_injection
            .as_ref()
            .expect("section")
            .channels
            .get("trae")
            .expect("channel");
        assert_eq!(channel.rules[0].target_header, "X-Opencode-Session");
    }

    #[test]
    fn header_injection_enable_with_fallback_sets_and_disable_keeps_it() {
        let mut config = token_proxy_config::ProxyConfigFile::default();
        mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Enable {
                channel: "trae".to_string(),
                header: None,
                fallback: true,
            },
        )
        .expect("enable");
        assert!(
            config
                .header_injection
                .as_ref()
                .expect("section")
                .channels
                .get("trae")
                .expect("channel")
                .fallback_enabled
        );

        mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Disable {
                channel: "trae".to_string(),
            },
        )
        .expect("disable");
        let channel = config
            .header_injection
            .as_ref()
            .expect("section")
            .channels
            .get("trae")
            .expect("channel kept");
        assert!(!channel.enabled);
        assert!(channel.fallback_enabled);
    }

    #[test]
    fn header_injection_disable_keeps_record() {
        let mut config = token_proxy_config::ProxyConfigFile::default();
        mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Enable {
                channel: "trae".to_string(),
                header: Some("X-Opencode-Session".to_string()),
                fallback: false,
            },
        )
        .expect("enable");

        let (changed, message) = mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Disable {
                channel: "trae".to_string(),
            },
        )
        .expect("disable");
        assert!(changed);
        assert!(message.contains("record kept"));
        let channel = config
            .header_injection
            .as_ref()
            .expect("section")
            .channels
            .get("trae")
            .expect("channel kept");
        assert!(!channel.enabled);
        assert_eq!(channel.rules[0].target_header, "X-Opencode-Session");
    }

    #[test]
    fn header_injection_disable_without_record_is_noop() {
        let mut config = token_proxy_config::ProxyConfigFile::default();
        let (changed, message) = mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Disable {
                channel: "trae".to_string(),
            },
        )
        .expect("disable");
        assert!(!changed);
        assert!(message.contains("nothing to do"));
        assert!(config.header_injection.is_none());
    }

    #[test]
    fn header_injection_enable_rejects_invalid_header() {
        let mut config = token_proxy_config::ProxyConfigFile::default();
        let error = mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Enable {
                channel: "trae".to_string(),
                header: Some("bad header".to_string()),
                fallback: false,
            },
        )
        .expect_err("invalid header");
        assert!(error.contains("invalid header name"));
        assert!(config.header_injection.is_none());
    }

    #[test]
    fn header_injection_rows_mark_orphans_and_state() {
        let mut config = token_proxy_config::ProxyConfigFile::default();
        mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Enable {
                channel: "trae".to_string(),
                header: None,
                fallback: false,
            },
        )
        .expect("enable");

        let section = config.header_injection.as_ref().expect("section");
        let rows = header_injection_rows(&config, section);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("trae"));
        assert!(rows[0].contains("hook_session_marker"));
        assert!(rows[0].contains("X-Session-ID"));
        assert!(rows[0].contains("(channel not found)"));

        mutate_header_injection(
            &mut config,
            HeaderInjectionCommand::Disable {
                channel: "trae".to_string(),
            },
        )
        .expect("disable");
        let section = config.header_injection.as_ref().expect("section");
        let rows = header_injection_rows(&config, section);
        assert!(rows[0].contains("false"));
    }
    // ══════════ HEADER-INJECTION PATCH 11 (cli tests) END ══════════
}
