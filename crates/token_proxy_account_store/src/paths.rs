use std::path::{Path, PathBuf};

pub const CONFIG_FILE_NAME: &str = "config.jsonc";
pub const DB_FILE_NAME: &str = "data.db";
/// SQLite WAL 旁路文件；计入数据库占用。
pub const DB_WAL_FILE_NAME: &str = "data.db-wal";
/// SQLite shared-memory 旁路文件；计入数据库占用。
pub const DB_SHM_FILE_NAME: &str = "data.db-shm";
// ══════════ RATE-RECORDER PATCH 1 (path) START ══════════
/// 请求级速率旁路记录库（fork 本地增强）；与 data.db 同数据目录。
pub const RATE_DB_FILE_NAME: &str = "rate.db";
// ══════════ RATE-RECORDER PATCH 1 (path) END ══════════

/// Token Proxy 的路径集合（Ports & Adapters 的最小“路径端口”）。
///
/// - **Core** 只依赖该结构来定位配置/数据文件；
/// - CLI/Tauri 通过不同构造方法把“路径策略”注入进来。
#[derive(Clone, Debug)]
pub struct TokenProxyPaths {
    config_file: PathBuf,
    data_dir: PathBuf,
}

impl TokenProxyPaths {
    /// 从配置文件路径构造；相对路径会按进程当前工作目录解析为绝对路径。
    ///
    /// 约定：数据目录默认使用配置文件所在目录（与 Tauri 的 app_config_dir 模型对齐），
    /// 使 `config.jsonc` / `data.db` / `*-auth/` 等文件天然“同目录聚合”。
    pub fn from_config_path(config_path: impl AsRef<Path>) -> Result<Self, String> {
        let config_path = config_path.as_ref();
        let cwd = std::env::current_dir().map_err(|err| format!("Failed to resolve cwd: {err}"))?;
        let config_file = if config_path.is_absolute() {
            config_path.to_path_buf()
        } else {
            cwd.join(config_path)
        };
        let data_dir = config_file
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .map(PathBuf::from)
            .unwrap_or(cwd);
        Ok(Self {
            config_file,
            data_dir,
        })
    }

    /// 从应用数据目录构造（例如：Tauri 的 app_config_dir）。
    ///
    /// - 配置文件固定为 `{data_dir}/config.jsonc`
    /// - 数据库固定为 `{data_dir}/data.db`
    pub fn from_app_data_dir(data_dir: PathBuf) -> Result<Self, String> {
        if data_dir.as_os_str().is_empty() {
            return Err("App data dir is required.".to_string());
        }
        Ok(Self {
            config_file: data_dir.join(CONFIG_FILE_NAME),
            data_dir,
        })
    }

    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn sqlite_db_path(&self) -> PathBuf {
        self.data_dir.join(DB_FILE_NAME)
    }

    // ══════════ RATE-RECORDER PATCH 1 (path) START ══════════
    /// 速率旁路库路径：`{data_dir}/rate.db`（与主库同目录的独立库）。
    pub fn rate_db_path(&self) -> PathBuf {
        self.data_dir.join(RATE_DB_FILE_NAME)
    }
    // ══════════ RATE-RECORDER PATCH 1 (path) END ══════════
}
