use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::services::ServeDir;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct AppState {
    root: PathBuf,
}

#[tokio::main]
async fn main() {
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8999".to_string());
    let data_root = std::env::var("DATA_ROOT").unwrap_or_else(|_| "/data".to_string());
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "static".to_string());

    let root_path = PathBuf::from(&data_root);
    let _ = fs::create_dir_all(&root_path);
    let root = root_path.canonicalize().unwrap_or(root_path);

    let app = Router::new()
        .route("/api/config", get(api_config))
        .route("/api/dirs", post(api_dirs))
        .route("/api/list", post(api_list))
        .route("/api/preview", post(api_preview))
        .route("/api/rename", post(api_rename))
        .fallback_service(ServeDir::new(static_dir).append_index_html_on_directories(true))
        .with_state(AppState { root });

    let addr: std::net::SocketAddr = bind.parse().expect("BIND_ADDR 格式错误");
    println!("file-renamer listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("端口绑定失败");
    axum::serve(listener, app).await.expect("服务启动失败");
}

fn ok_json(data: serde_json::Value) -> Response {
    Json(json!({ "ok": true, "data": data })).into_response()
}

fn err_json(msg: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "error": msg.into() })),
    )
        .into_response()
}

async fn api_config(State(st): State<AppState>) -> Response {
    ok_json(json!({
        "root": st.root.to_string_lossy(),
        "version": env!("CARGO_PKG_VERSION"),
        "default_port": 8999
    }))
}

/* ---------------- 路径安全 ---------------- */

fn resolve_dir(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim().trim_start_matches('/');
    let raw = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let canon = raw
        .canonicalize()
        .map_err(|e| format!("目录不可访问: {} ({})", raw.display(), e))?;
    if !canon.starts_with(root) {
        return Err("路径超出允许范围".to_string());
    }
    if !canon.is_dir() {
        return Err("目标不是目录".to_string());
    }
    Ok(canon)
}

fn resolve_item(root: &Path, rel: &str) -> Result<PathBuf, String> {
    // 仅去掉前导斜杠，保留文件名首尾空格（空格本身可作为删除规则的关键字）
    let rel_clean = rel.trim_start_matches('/');
    if rel_clean.is_empty() {
        return Err("非法的空路径".to_string());
    }
    let raw = root.join(rel_clean);
    let canon = raw
        .canonicalize()
        .map_err(|e| format!("路径不可访问: {} ({})", rel, e))?;
    if !canon.starts_with(root) {
        return Err(format!("路径超出允许范围: {}", rel));
    }
    Ok(canon)
}

fn rel_of(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(p) => {
            let s = p.to_string_lossy().replace('\\', "/");
            if s.is_empty() {
                "/".to_string()
            } else {
                s
            }
        }
        Err(_) => path.to_string_lossy().replace('\\', "/"),
    }
}

/* ---------------- 列表扫描 ---------------- */

#[derive(Deserialize)]
struct ListReq {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    keyword: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default = "default_true")]
    recursive: bool,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_true() -> bool {
    true
}

fn default_limit() -> usize {
    5000
}

#[derive(Serialize)]
struct Item {
    name: String,
    rel: String,
    size: u64,
    modified: i64,
    is_dir: bool,
}

async fn api_list(State(st): State<AppState>, Json(req): Json<ListReq>) -> Response {
    match list_items(&st.root, req) {
        Ok(v) => ok_json(v),
        Err(e) => err_json(e),
    }
}

fn list_items(root: &Path, req: ListReq) -> Result<serde_json::Value, String> {
    let base = resolve_dir(root, req.root.as_deref().unwrap_or(""))?;
    let keyword = req.keyword.unwrap_or_default();
    let kind = req.kind.unwrap_or_else(|| "file".to_string());
    let want_file = kind != "dir";
    let want_dir = kind != "file";
    let limit = req.limit.clamp(1, 20000);

    let (items, truncated) = scan(
        &base,
        root,
        &keyword,
        req.case_sensitive,
        want_file,
        want_dir,
        req.recursive,
        limit,
    );

    Ok(json!({
        "root": rel_of(root, &base),
        "items": items,
        "total": items.len(),
        "truncated": truncated
    }))
}

#[allow(clippy::too_many_arguments)]
fn scan(
    base: &Path,
    root: &Path,
    keyword: &str,
    case_sensitive: bool,
    want_file: bool,
    want_dir: bool,
    recursive: bool,
    limit: usize,
) -> (Vec<Item>, bool) {
    let mut out: Vec<Item> = Vec::new();
    let mut truncated = false;
    let needle = if case_sensitive {
        keyword.to_string()
    } else {
        keyword.to_lowercase()
    };

    let mut stack: Vec<PathBuf> = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut entries: Vec<fs::DirEntry> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by(|a, b| {
            let an = a.file_name().to_string_lossy().to_string();
            let bn = b.file_name().to_string_lossy().to_string();
            natcmp(&an, &bn)
        });

        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            // 跳过符号链接，避免越界与递归死循环
            if ft.is_symlink() {
                continue;
            }
            let is_dir = ft.is_dir();
            let meta = entry.metadata().ok();

            let hay = if case_sensitive {
                name.clone()
            } else {
                name.to_lowercase()
            };
            let hit = needle.is_empty() || hay.contains(needle.as_str());

            if hit && ((is_dir && want_dir) || (!is_dir && want_file)) {
                if out.len() >= limit {
                    truncated = true;
                } else {
                    let modified = meta
                        .as_ref()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    out.push(Item {
                        name,
                        rel: rel_of(root, &path),
                        size: if is_dir {
                            0
                        } else {
                            meta.as_ref().map(|m| m.len()).unwrap_or(0)
                        },
                        modified,
                        is_dir,
                    });
                }
            }

            if is_dir && recursive {
                stack.push(path);
            }
        }
    }

    (out, truncated)
}

/* ---------------- 目录树（懒加载） ---------------- */

#[derive(Deserialize)]
struct DirsReq {
    #[serde(default)]
    root: Option<String>,
}

async fn api_dirs(State(st): State<AppState>, Json(req): Json<DirsReq>) -> Response {
    match list_dirs(&st.root, req) {
        Ok(v) => ok_json(v),
        Err(e) => err_json(e),
    }
}

fn list_dirs(root: &Path, req: DirsReq) -> Result<serde_json::Value, String> {
    let base = resolve_dir(root, req.root.as_deref().unwrap_or(""))?;
    let rd = fs::read_dir(&base).map_err(|e| format!("目录不可访问: {} ({})", base.display(), e))?;
    let mut dirs: Vec<serde_json::Value> = Vec::new();
    for entry in rd.filter_map(|e| e.ok()) {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        // 跳过符号链接，避免越界与递归死循环
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let has_children = fs::read_dir(&path)
            .map(|children| {
                children.filter_map(|c| c.ok()).any(|c| {
                    c.file_type()
                        .map(|t| t.is_dir() && !t.is_symlink())
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        dirs.push(json!({
            "name": name,
            "rel": rel_of(root, &path),
            "has_children": has_children
        }));
    }
    dirs.sort_by(|a, b| {
        let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        natcmp(an, bn)
    });
    Ok(json!({
        "root": rel_of(root, &base),
        "dirs": dirs,
        "total": dirs.len()
    }))
}

/* ---------------- 重命名规则 ---------------- */

#[derive(Deserialize)]
struct RenameReq {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    items: Vec<String>,
    rule: Rule,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Deserialize, Default, Clone)]
struct Rule {
    /// 新格式：动态规则列表，数组顺序即执行顺序
    #[serde(default)]
    rules: Vec<serde_json::Value>,
    /// 规则作用对象是否包含扩展名，默认 false（保持旧行为：扩展名始终保留、规则只作用于主名）
    #[serde(default)]
    include_ext: bool,
    // ---- 以下为旧版固定字段，保留用于向后兼容 ----
    #[serde(default)]
    serial: Option<Serial>,
    #[serde(default)]
    remove: Option<RemoveRule>,
    #[serde(default)]
    replace: Option<ReplaceRule>,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    suffix: Option<String>,
}

/// 单条动态规则。`type` 取值：serial / remove / replace / cut / prefix / suffix
#[derive(Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RuleKind {
    Serial {
        #[serde(default = "one")]
        start: u64,
        #[serde(default = "one")]
        step: u64,
        #[serde(default = "three")]
        padding: usize,
        #[serde(default = "underscore")]
        separator: String,
        #[serde(default = "prefix_pos")]
        position: String,
    },
    Remove {
        /// keyword（默认）| cut
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        keyword: String,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        case_sensitive: bool,
        /// mode=cut 时：保留前 N 位字符，0 表示全删；
        /// mode=cut_left 时：删除前 N 位字符；
        /// mode=cut_right 时：删除后 N 位字符
        #[serde(default)]
        position: Option<usize>,
    },
    Cut {
        #[serde(default)]
        position: usize,
    },
    Replace {
        #[serde(rename = "from")]
        from: String,
        #[serde(rename = "to")]
        to: String,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        case_sensitive: bool,
    },
    Prefix {
        #[serde(default)]
        value: String,
    },
    Suffix {
        #[serde(default)]
        value: String,
    },
}

/// 合并新旧两种入参，得到有序规则列表；旧格式按历史执行顺序转换：
/// 删除关键字 → 替换关键字 → 前缀 → 后缀 → 序列号
fn effective_ops(rule: &Rule) -> Result<Vec<RuleKind>, String> {
    if !rule.rules.is_empty() {
        let mut ops = Vec::with_capacity(rule.rules.len());
        for (i, v) in rule.rules.iter().enumerate() {
            if v.get("enabled").and_then(|x| x.as_bool()) == Some(false) {
                continue;
            }
            let op: RuleKind = serde_json::from_value(v.clone())
                .map_err(|e| format!("第 {} 条规则参数错误: {}", i + 1, e))?;
            ops.push(op);
        }
        return Ok(ops);
    }

    let mut ops = Vec::new();
    if let Some(r) = &rule.remove {
        if !r.keyword.is_empty() {
            ops.push(RuleKind::Remove {
                mode: None,
                keyword: r.keyword.clone(),
                all: r.all,
                case_sensitive: r.case_sensitive,
                position: None,
            });
        }
    }
    if let Some(r) = &rule.replace {
        if !r.from.is_empty() {
            ops.push(RuleKind::Replace {
                from: r.from.clone(),
                to: r.to.clone(),
                all: r.all,
                case_sensitive: r.case_sensitive,
            });
        }
    }
    if let Some(p) = &rule.prefix {
        if !p.is_empty() {
            ops.push(RuleKind::Prefix { value: p.clone() });
        }
    }
    if let Some(s) = &rule.suffix {
        if !s.is_empty() {
            ops.push(RuleKind::Suffix { value: s.clone() });
        }
    }
    if let Some(s) = &rule.serial {
        ops.push(RuleKind::Serial {
            start: s.start,
            step: s.step,
            padding: s.padding,
            separator: s.separator.clone(),
            position: s.position.clone(),
        });
    }
    Ok(ops)
}

#[derive(Deserialize, Clone)]
struct Serial {
    #[serde(default = "one")]
    start: u64,
    #[serde(default = "one")]
    step: u64,
    #[serde(default = "three")]
    padding: usize,
    #[serde(default = "underscore")]
    separator: String,
    #[serde(default = "prefix_pos")]
    position: String,
}

fn one() -> u64 {
    1
}
fn three() -> usize {
    3
}
fn underscore() -> String {
    "_".to_string()
}
fn prefix_pos() -> String {
    "prefix".to_string()
}

#[derive(Deserialize, Clone)]
struct RemoveRule {
    keyword: String,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    case_sensitive: bool,
}

#[derive(Deserialize, Clone)]
struct ReplaceRule {
    #[serde(rename = "from")]
    from: String,
    #[serde(rename = "to")]
    to: String,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    case_sensitive: bool,
}

struct Change {
    from_rel: String,
    from_abs: PathBuf,
    to_rel: String,
    to_abs: PathBuf,
    to_name: String,
    ok: bool,
    error: Option<String>,
    tmp: Option<PathBuf>,
}

async fn api_preview(State(st): State<AppState>, Json(req): Json<RenameReq>) -> Response {
    match plan(&st.root, &req) {
        Ok(changes) => ok_json(changes_json(&changes, true)),
        Err(e) => err_json(e),
    }
}

async fn api_rename(State(st): State<AppState>, Json(req): Json<RenameReq>) -> Response {
    if req.dry_run {
        return match plan(&st.root, &req) {
            Ok(changes) => ok_json(changes_json(&changes, true)),
            Err(e) => err_json(e),
        };
    }
    match plan(&st.root, &req) {
        Ok(mut changes) => {
            apply_rename(&mut changes);
            ok_json(changes_json(&changes, false))
        }
        Err(e) => err_json(e),
    }
}

fn changes_json(changes: &[Change], dry_run: bool) -> serde_json::Value {
    let items: Vec<serde_json::Value> = changes
        .iter()
        .map(|c| {
            json!({
                "from": c.from_rel,
                "to": c.to_rel,
                "to_name": c.to_name,
                "ok": c.ok,
                "changed": c.error.is_none() && !c.to_name.is_empty(),
                "error": c.error,
            })
        })
        .collect();
    let failed = changes.iter().filter(|c| c.error.is_some()).count();
    let success = changes.iter().filter(|c| c.ok).count();
    json!({
        "dry_run": dry_run,
        "items": items,
        "total": changes.len(),
        "success": success,
        "failed": failed
    })
}

fn plan(root: &Path, req: &RenameReq) -> Result<Vec<Change>, String> {
    if req.items.is_empty() {
        return Err("请先选择要重命名的文件或文件夹".to_string());
    }
    let base = resolve_dir(root, req.root.as_deref().unwrap_or(""))?;

    let mut entries: Vec<(String, PathBuf, bool)> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for rel in &req.items {
        let abs = resolve_item(root, rel)?;
        if !abs.starts_with(&base) {
            return Err(format!("{} 不在当前目录范围内", rel));
        }
        if !seen.insert(abs.clone()) {
            continue;
        }
        let is_dir = abs.is_dir();
        entries.push((rel.clone(), abs, is_dir));
    }
    if entries.is_empty() {
        return Err("没有有效的待处理项".to_string());
    }

    // 禁止同时勾选父文件夹与其子项，否则父目录改名后子项路径会失效
    for i in 0..entries.len() {
        if !entries[i].2 {
            continue;
        }
        for j in 0..entries.len() {
            if i != j && entries[j].1 != entries[i].1 && entries[j].1.starts_with(&entries[i].1) {
                return Err(format!(
                    "不能同时选择父文件夹与其内部项目：{}",
                    entries[i].0
                ));
            }
        }
    }

    entries.sort_by(|a, b| natcmp(&a.0, &b.0));

    let src_set: HashSet<PathBuf> = entries.iter().map(|e| e.1.clone()).collect();
    let mut target_set: HashSet<PathBuf> = HashSet::new();
    let mut changes: Vec<Change> = Vec::with_capacity(entries.len());

    for (idx, (rel, abs, is_dir)) in entries.iter().enumerate() {
        let name = abs
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let parent = abs.parent().unwrap_or(root).to_path_buf();
        let parent_rel = Path::new(rel)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        let mut change = Change {
            from_rel: rel.clone(),
            from_abs: abs.clone(),
            to_rel: String::new(),
            to_abs: abs.clone(),
            to_name: String::new(),
            ok: false,
            error: None,
            tmp: None,
        };

        match build_new_name(&name, *is_dir, &req.rule, idx as u64) {
            Err(e) => {
                change.error = Some(e);
            }
            Ok(new_name) => {
                if new_name == name {
                    change.error = Some("名称未发生变化".to_string());
                    change.to_name = new_name.clone();
                    change.to_rel = rel.clone();
                    changes.push(change);
                    continue;
                }
                if new_name.len() > 255 {
                    change.error = Some("新文件名超过 255 字节".to_string());
                    changes.push(change);
                    continue;
                }
                let target_abs = parent.join(&new_name);
                let to_rel = if parent_rel.is_empty() || parent_rel == "/" {
                    new_name.clone()
                } else {
                    format!("{}/{}", parent_rel.trim_end_matches('/'), new_name)
                };

                if target_abs.exists() && !src_set.contains(&target_abs) {
                    change.error = Some("目标已存在，已跳过".to_string());
                } else if !target_set.insert(target_abs.clone()) {
                    change.error = Some("与其他项的目标名称重复".to_string());
                }

                change.to_name = new_name;
                change.to_rel = to_rel;
                change.to_abs = target_abs;
            }
        }

        changes.push(change);
    }

    Ok(changes)
}

fn split_name(name: &str, is_dir: bool, include_ext: bool) -> (String, String) {
    // 目录没有扩展名概念；勾选“包含扩展名”时整个文件名都参与规则计算
    if is_dir || include_ext {
        return (name.to_string(), String::new());
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

/// 保留前 n 个字符（按字符计数，避免截断中文/emoji），删除其余部分；n=0 表示全删
fn cut_after(s: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    s.chars().take(n).collect()
}

/// 删除前 n 个字符，保留其余部分；n=0 表示不删除
fn cut_before(s: &str, n: usize) -> String {
    if n == 0 {
        return s.to_string();
    }
    s.chars().skip(n).collect()
}

/// 删除后 n 个字符；n=0 表示不删除，n 大于等于字符总数时全删
fn cut_tail(s: &str, n: usize) -> String {
    if n == 0 {
        return s.to_string();
    }
    let total = s.chars().count();
    if n >= total {
        return String::new();
    }
    s.chars().take(total - n).collect()
}

fn build_new_name(name: &str, is_dir: bool, rule: &Rule, idx: u64) -> Result<String, String> {
    let (stem, ext) = split_name(name, is_dir, rule.include_ext);
    let mut base = stem;

    for op in effective_ops(rule)? {
        match op {
            RuleKind::Remove {
                mode,
                keyword,
                all,
                case_sensitive,
                position,
            } => {
                let m = mode.as_deref().unwrap_or("keyword");
                if m == "cut_left" {
                    let n = position.ok_or_else(|| "删除前字符数参数缺失".to_string())?;
                    base = cut_before(&base, n);
                } else if m == "cut_right" {
                    let n = position.ok_or_else(|| "删除后字符数参数缺失".to_string())?;
                    base = cut_tail(&base, n);
                } else if m == "cut" {
                    let n = position.ok_or_else(|| "删除位置参数缺失".to_string())?;
                    base = cut_after(&base, n);
                } else {
                    let kw = keyword.as_str();
                    if !kw.is_empty() {
                        base = if all {
                            replace_all(&base, kw, "", case_sensitive)
                        } else {
                            replace_first(&base, kw, "", case_sensitive)
                        };
                    }
                }
            }
            RuleKind::Cut { position } => {
                base = cut_after(&base, position);
            }
            RuleKind::Replace {
                from,
                to,
                all,
                case_sensitive,
            } => {
                if !from.is_empty() {
                    base = if all {
                        replace_all(&base, &from, &to, case_sensitive)
                    } else {
                        replace_first(&base, &from, &to, case_sensitive)
                    };
                }
            }
            RuleKind::Prefix { value } => {
                if !value.is_empty() {
                    base = format!("{}{}", value, base);
                }
            }
            RuleKind::Suffix { value } => {
                if !value.is_empty() {
                    base = format!("{}{}", base, value);
                }
            }
            RuleKind::Serial {
                start,
                step,
                padding,
                separator,
                position,
            } => {
                let num = start.saturating_add(step.saturating_mul(idx));
                let num_str = format!("{:0width$}", num, width = padding.max(1));
                if position == "suffix" {
                    base = format!("{}{}{}", base, separator, num_str);
                } else {
                    base = format!("{}{}{}", num_str, separator, base);
                }
            }
        }
    }

    // 主名被规则清空时直接报错：N=0 的“全删”会走到这里，避免生成仅剩扩展名的隐藏文件
    if base.trim().is_empty() {
        return Err("规则执行后文件名为空（删除的字符数过多或为 0 会清空全部字符）".to_string());
    }

    let new_name = format!("{}{}", base, ext);
    let trimmed = new_name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err("规则执行后文件名为空".to_string());
    }
    if new_name.contains('/') || new_name.contains('\\') || new_name.contains('\0') {
        return Err("新文件名包含非法字符".to_string());
    }
    Ok(new_name)
}

fn find_pos(s: &str, needle: &str, case_sensitive: bool) -> Option<(usize, usize)> {
    if case_sensitive {
        s.find(needle).map(|p| (p, needle.len()))
    } else {
        let hay = s.to_lowercase();
        let nd = needle.to_lowercase();
        hay.find(nd.as_str()).and_then(|p| {
            let end = p + nd.len();
            if end <= s.len() && s.is_char_boundary(p) && s.is_char_boundary(end) {
                Some((p, nd.len()))
            } else {
                None
            }
        })
    }
}

fn replace_first(s: &str, from: &str, to: &str, case_sensitive: bool) -> String {
    if from.is_empty() {
        return s.to_string();
    }
    match find_pos(s, from, case_sensitive) {
        Some((start, len)) if len > 0 => {
            let mut out = String::with_capacity(s.len());
            out.push_str(&s[..start]);
            out.push_str(to);
            out.push_str(&s[start + len..]);
            out
        }
        _ => s.to_string(),
    }
}

fn replace_all(s: &str, from: &str, to: &str, case_sensitive: bool) -> String {
    if from.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match find_pos(rest, from, case_sensitive) {
            Some((p, len)) if len > 0 => {
                out.push_str(&rest[..p]);
                out.push_str(to);
                rest = &rest[p + len..];
                if rest.is_empty() {
                    break;
                }
            }
            _ => break,
        }
    }
    out.push_str(rest);
    out
}

fn apply_rename(changes: &mut [Change]) {
    let mut staged: Vec<usize> = Vec::new();

    // 第一阶段：全部改名为同目录下的临时名，避免 A->B、B->C 之类的链式冲突
    for i in 0..changes.len() {
        if changes[i].error.is_some() {
            continue;
        }
        let parent = match changes[i].from_abs.parent() {
            Some(p) => p.to_path_buf(),
            None => {
                changes[i].error = Some("无法定位父目录".to_string());
                continue;
            }
        };
        let tmp_name = format!(
            ".renamer_tmp_{}_{}_{}",
            std::process::id(),
            now_nanos(),
            TMP_COUNTER.fetch_add(1, AtomicOrdering::SeqCst)
        );
        let tmp = parent.join(tmp_name);
        match fs::rename(&changes[i].from_abs, &tmp) {
            Ok(_) => {
                changes[i].tmp = Some(tmp);
                staged.push(i);
            }
            Err(e) => {
                changes[i].error = Some(format!("准备改名失败: {}", e));
            }
        }
    }

    // 第二阶段：临时名 -> 目标名，失败则回滚
    for i in staged {
        let tmp = match changes[i].tmp.take() {
            Some(t) => t,
            None => continue,
        };
        // Linux 的 rename 会静默覆盖同名目标，落盘前再校验一次
        if changes[i].to_abs.exists() {
            let _ = fs::rename(&tmp, &changes[i].from_abs);
            changes[i].error = Some("目标已存在，已跳过".to_string());
            continue;
        }
        match fs::rename(&tmp, &changes[i].to_abs) {
            Ok(_) => {
                changes[i].ok = true;
            }
            Err(e) => {
                let _ = fs::rename(&tmp, &changes[i].from_abs);
                changes[i].error = Some(format!("重命名失败: {}", e));
            }
        }
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn natcmp(a: &str, b: &str) -> Ordering {
    let au: Vec<char> = a.to_lowercase().chars().collect();
    let bu: Vec<char> = b.to_lowercase().chars().collect();
    let (mut i, mut j) = (0usize, 0usize);

    while i < au.len() && j < bu.len() {
        if au[i].is_ascii_digit() && bu[j].is_ascii_digit() {
            let mut i2 = i;
            while i2 < au.len() && au[i2].is_ascii_digit() {
                i2 += 1;
            }
            let mut j2 = j;
            while j2 < bu.len() && bu[j2].is_ascii_digit() {
                j2 += 1;
            }
            let n1: String = au[i..i2].iter().collect();
            let n2: String = bu[j..j2].iter().collect();
            let t1 = n1.trim_start_matches('0');
            let t2 = n2.trim_start_matches('0');
            let c = t1.len().cmp(&t2.len()).then_with(|| t1.cmp(t2));
            if c != Ordering::Equal {
                return c;
            }
            i = i2;
            j = j2;
        } else {
            if au[i] != bu[j] {
                return au[i].cmp(&bu[j]);
            }
            i += 1;
            j += 1;
        }
    }

    au.len().saturating_sub(i).cmp(&bu.len().saturating_sub(j))
}
