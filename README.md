---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 8fdee0d405a640a32471d6b1a1ccced5_5f30b7b2d7fe11f18947525400de85a5
    ReservedCode1: vxz65dNzAbUZHSJNuSBkuneoaLm8D2EBFOLtRjZhVfUpq3CCc/OCQnXVaYdqLLZLsNfwkJ7p0dOjG7mbO+sRIZ6I/KO5wkuX9+48YUbMY3z8CJUC6MG7XXxxrZcd4gYqT0bADKCFXIZtMQYEEfdtW4cacwni2ZNjaViMjDB0dalXcJintzmnMFficSY=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 8fdee0d405a640a32471d6b1a1ccced5_5f30b7b2d7fe11f18947525400de85a5
    ReservedCode2: vxz65dNzAbUZHSJNuSBkuneoaLm8D2EBFOLtRjZhVfUpq3CCc/OCQnXVaYdqLLZLsNfwkJ7p0dOjG7mbO+sRIZ6I/KO5wkuX9+48YUbMY3z8CJUC6MG7XXxxrZcd4gYqT0bADKCFXIZtMQYEEfdtW4cacwni2ZNjaViMjDB0dalXcJintzmnMFficSY=
---

# 文件批量重命名（Rust + Web）

一个用 Rust 编写的本地批量重命名工具，自带 Web 界面，通过 Docker 一条命令即可运行，默认端口 **8999**。

## 功能

- **两大分类**：文件 / 文件夹，各自独立筛选与批量处理
- **目录选择**：可展开、可点选的目录树（懒加载，仅展示 `/data` 内目录），同时保留手动输入相对路径
- **筛选**：指定根目录 + 子路径、按名称关键字搜索、是否递归子目录、是否区分大小写
- **动态规则列表（可任意组合）**
  - 可添加多条规则，支持上移 / 下移排序、删除、单条启用 / 停用
  - 每条规则可展开设置详细参数，**列表顺序即执行顺序**
  - 规则类型：
    - 序列化编号：起始值、步长、补零位数、放在名称前或后、自定义分隔符
    - 删除关键字：全部删除或只删第一个匹配
    - **删除前 N 位字符**：从名称开头删除指定数量的字符（按字符计数，支持中文/emoji），0 表示不删除
    - **删除后 N 位字符**：从名称末尾删除指定数量的字符，0 表示不删除
    - 替换关键字：全部替换或只替换第一个匹配
    - 增加前缀 / 后缀
- **包含扩展名开关**：控制规则作用对象是否包含扩展名。默认关闭（规则只作用于主名、扩展名始终保留，与旧版行为一致）；开启后删除 / 截断 / 替换 / 前缀 / 后缀 / 序号均作用于完整文件名
- **预览**：执行前展示「原名 → 新名」，并提示冲突
- **安全**：所有路径限制在挂载目录内；目标已存在、重名、名称未变化的项自动跳过并回报；两阶段改名避免 `A→B、B→C` 链式冲突

## 目录结构

```
file-renamer/
├── Cargo.toml
├── Cargo.lock           # 锁定依赖版本，已随项目提交
├── Dockerfile
├── docker-compose.yml
├── src/
│   └── main.rs          # Axum HTTP 服务 + 扫描/规则/改名逻辑
└── static/
    └── index.html       # 单页 Web 界面（原生 JS，无构建步骤）
```

## 快速开始

```bash
# 在项目目录下
docker compose up -d --build
```

打开浏览器访问 <http://localhost:8999>。

`docker-compose.yml` 中的卷需要改成宿主机上真正要管理的目录：

```yaml
volumes:
  - /path/to/your/media:/data   # 宿主机目录映射到容器内 /data
```

界面上「根目录」填写的是 **相对 `/data` 的子路径**，留空表示 `/data` 本身，例如 `photos/2024`。

## 本地运行（可选）

**下载预编译二进制（无需 Docker）**：从 Release 下载 `file-renamer-0.2.0-x86_64-linux-gnu.tar.gz`，解压后 `static/` 目录必须与二进制同级（或用 `STATIC_DIR` 指定）：

```bash
tar xzf file-renamer-0.2.0-x86_64-linux-gnu.tar.gz
cd file-renamer-0.2.0
DATA_ROOT=/path/to/your/media ./file-renamer
# 打开 http://localhost:8999
```

要求 glibc ≥ 2.36（Debian 12 / Ubuntu 22.04+），更老的系统请走 Docker。

从源码构建需要 Rust 1.85+（依赖链中 tokio-util / hyper-util 已使用 edition 2024）：

```bash
cargo run --release
```

项目已提交 `Cargo.lock`，构建时会锁定依赖版本，不会因上游发布新版本而失效。更新依赖后请一并提交新的 `Cargo.lock`。

默认读取当前目录下的 `static/` 作为前端资源。

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `BIND_ADDR` | `0.0.0.0:8999` | 监听地址与端口 |
| `DATA_ROOT` | `/data` | 允许操作的根目录，所有路径不得越出此目录 |
| `STATIC_DIR` | `static` | 前端静态文件目录 |

## 使用流程

1. 点击「目录树」逐层展开并点选目标目录（也可直接在输入框填写相对 `/data` 的路径），点击「加载」
2. 切换「文件 / 文件夹」标签
3. 输入关键字筛选（留空显示全部），可选递归、区分大小写
4. 勾选要处理的项目（支持全选）
5. 在右侧从下拉框选择规则类型，点「+ 添加规则」；用 ↑ ↓ 调整顺序，点标题展开参数，勾选框启用 / 停用，✕ 删除
6. 按需勾选「规则作用于完整文件名（含扩展名）」
7. 点「预览」核对结果，确认无误后「确认执行」

### 规则执行顺序

规则按列表**自上而下**执行，顺序可自由调整。默认不含扩展名，等价于旧版：

```
删除 / 截断 → 替换关键字 → 加前缀 → 加后缀 → 追加序列号
```

默认情况下扩展名不参与规则并始终保留。例如文件 `IMG_2024_副本.jpg`，规则「删除 `_副本`」+「序列化 3 位、前缀、分隔符 `_`」在序号 1 时得到 `001_IMG_2024.jpg`。

勾选「包含扩展名」后完整名称参与计算。例如对 `IMG_2024_副本.jpg` 加后缀 `_final`：默认（不含扩展名）得到 `IMG_2024_副本_final.jpg`，勾选后得到 `IMG_2024_副本.jpg_final`；「删除第 N 位之后」也会按完整名称截断（可能连同扩展名一起去掉），请先预览确认。

序列号按**相对路径的自然排序**分配（`1, 2, 10` 而不是 `1, 10, 2`），与界面列表顺序一致。

### 冲突处理

以下情况该条目标记为跳过，不影响其他项：

- 目标名称已存在且不在本次改名集合中
- 多个条目算出同一个目标名称
- 规则执行后名称没有变化
- 新名称为空或包含 `/`、`\`

## API

所有接口使用 `POST` + JSON，返回统一结构：`{"ok": true, "data": ...}` 或 `{"ok": false, "error": "..."}`。

### `GET /api/config`

返回服务信息与根目录。

### `POST /api/list`

```json
{
  "root": "photos",          // 相对 DATA_ROOT，可空
  "keyword": "IMG",          // 可空
  "kind": "file",            // file | dir
  "recursive": true,
  "case_sensitive": false,
  "limit": 5000
}
```

### `POST /api/dirs`

目录树懒加载接口，仅返回 `DATA_ROOT` 内的直接子目录（跳过符号链接）。

```json
{
  "root": "photos"   // 相对 DATA_ROOT，可空表示根
}
```

返回：

```json
{
  "ok": true,
  "data": {
    "root": "photos",
    "dirs": [
      { "name": "2024", "rel": "photos/2024", "has_children": true }
    ],
    "total": 1
  }
}
```

### `POST /api/preview` 与 `POST /api/rename`

```json
{
  "root": "photos",
  "items": ["photos/IMG_1.jpg", "photos/IMG_2.jpg"],
  "rule": {
    "include_ext": false,
    "rules": [
      { "type": "remove", "mode": "keyword", "keyword": "_副本", "all": true, "case_sensitive": false },
      { "type": "cut", "position": 8 },
      { "type": "replace", "from": "IMG", "to": "PHOTO", "all": true, "case_sensitive": false },
      { "type": "prefix", "value": "2024_" },
      { "type": "suffix", "value": "_final" },
      { "type": "serial", "start": 1, "step": 1, "padding": 3, "separator": "_", "position": "prefix" }
    ]
  },
  "dry_run": true
}
```

- `rules` 为有序数组，**数组顺序即执行顺序**；`enabled: false` 的条目会被忽略。
- 规则类型：`serial` / `remove` / `replace` / `cut` / `prefix` / `suffix`。
  - `remove` 支持 `mode: "keyword"`（默认，按关键字删除）与 `mode: "cut"`（保留前 `position` 个字符，`0` 表示全删）。
  - 也可直接使用独立规则 `{ "type": "cut", "position": 8 }`。
- `include_ext` 默认 `false`，规则只作用于主名、扩展名保留；为 `true` 时作用于完整文件名。
- **向后兼容**：仍可提交旧格式（`serial` / `remove` / `replace` / `prefix` / `suffix` 平铺字段），服务端会按旧版顺序自动转换为规则列表。
- `/api/rename` 需传 `"dry_run": false` 才会真正修改文件。

## 安全说明

- 所有传入路径都会 `canonicalize` 后校验必须位于 `DATA_ROOT` 内，越界直接拒绝
- 符号链接不会被扫描，也不会被改名
- 允许改名的父文件夹与其子项不能同时提交
- 建议始终保持「先预览、后执行」的习惯；本工具不提供撤销功能，重要数据请先备份

## 备注

- 首次 `docker compose up --build` 需要联网拉取 `rust` 与 `debian` 基础镜像，并下载 crates 依赖，耗时取决于网络
- 镜像采用多阶段构建，最终运行时镜像仅包含二进制与静态资源，体积远小于构建镜像
*（内容由AI生成，仅供参考）*
