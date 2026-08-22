# Agent Context 配置与凭据访问设计

## 1. 概述

Agent Context 为本地 Agent 提供统一的用户环境信息入口。用户维护一份 TOML 文件，按 profile（环境分组）保存常用的 LLM endpoint、模型名称、CI tags、开发环境以及其他自定义配置。Agent 可以通过统一的 CLI 浏览、搜索和读取这些信息。

配置文件只保存普通配置和凭据引用。Token、密码、私钥等敏感内容由系统凭据存储、环境变量或密码管理器保存，并在需要时临时提供给目标程序。

## 2. 设计目标

- 用户只需维护一份易读、易编辑的配置文件。
- 用户可以增加任意自定义配置，无需修改 CLI 代码。
- 每个配置项都有简短描述，帮助 Agent 理解其用途。
- CLI 可以快速列出当前保存了哪些配置和字段。
- Agent 能以适合脚本处理的格式读取配置。
- 明文凭据不会进入配置文件、Agent 对话或日志。
- 配置缺失或凭据不可用时明确报错。

## 3. 配置文件

默认路径按平台确定：

- Unix 类系统：`$XDG_CONFIG_HOME/agent-context/context.toml`，未设置 `XDG_CONFIG_HOME` 时为 `~/.config/agent-context/context.toml`。
- Windows：`%APPDATA%\agent-context\context.toml`。

用户可以通过环境变量指定其他位置：

```text
AGENT_CONTEXT_FILE=/path/to/context.toml
```

在 Unix 类系统上，文件权限应为 `0600`，`validate` 会检查这一点（见第 5.7 节）。文件不应包含明文凭据，但可能包含内部服务地址和环境名称。

## 4. 配置结构

配置文件包含三个部分：

- 全局设置：配置版本和默认 profile。
- `profiles`：用户的环境分组及其配置项。
- `credentials`：凭据的保存位置和取用方式。

完整示例：

```toml
version = 1
default_profile = "work"

[profiles.work]
description = "公司项目的日常开发环境。"

[profiles.work.llm]
description = "公司项目默认使用的 LLM。"
endpoint = "https://llm.example.com/v1"
model = "company-model"
credential = "credential://company_llm"

[profiles.work.llm.inject]
OPENAI_BASE_URL = "endpoint"
OPENAI_MODEL = "model"

[profiles.work.ci]
description = "公司项目提交 CI 任务时使用的标签。"
tags = ["linux", "self-hosted"]

[profiles.work.kubernetes]
description = "日常开发使用的 Kubernetes 测试环境。"
context = "company-staging"
namespace = "developer-tools"

[profiles.personal]
description = "个人项目使用的环境。"

[profiles.personal.llm]
description = "个人项目默认使用的公共 LLM。"
endpoint = "https://api.openai.com/v1"
model = "gpt-5"
credential = "credential://openai_personal"

[credentials.company_llm]
description = "公司 LLM 的访问凭据。"
provider = "env"
name = "COMPANY_LLM_TOKEN"
inject_as = "OPENAI_API_KEY"

[credentials.openai_personal]
description = "个人 OpenAI 账户的访问凭据。"
provider = "keychain"
service = "agent-context"
account = "openai-personal"
inject_as = "OPENAI_API_KEY"
```

### 4.1 Profile

Profile 表示一组相关的用户环境，例如 `work`、`personal` 或 `ci`。

每个 profile 必须包含 `description`：

```toml
[profiles.work]
description = "公司项目的日常开发环境。"
```

CLI 按以下顺序选择当前 profile：

1. 命令行参数 `--profile`。
2. 环境变量 `AGENT_CONTEXT_PROFILE`。
3. 配置文件中的 `default_profile`。

如果最终无法确定 profile，CLI 应明确报错并列出可用项。

### 4.2 自定义配置项

Profile 下的每个一级子表都是一个配置项。配置项必须包含 `description`，其他字段完全由用户定义：

```toml
[profiles.work.remote_cache]
description = "编译大型项目时使用的远程缓存。"
endpoint = "https://cache.example.com"
enabled = true
max_connections = 8
tags = ["build", "cache"]
```

用户可以使用 TOML 支持的字符串、整数、浮点数、布尔值、日期时间、数组和子表。CLI 通过通用路径遍历配置，因此新增字段后无需更新程序。

`description` 只说明该配置项是什么以及何时使用。字段的实际值仍保存在同一个配置项中，避免引入额外的元数据层级。

保留键名：

- `description` 在 profile 和配置项两级都是保留字段，用户不能创建名为 `description` 的配置项或字段。
- `inject` 在配置项一级是保留子表（见第 4.4 节），用户不能将它用作普通字段。

### 4.3 凭据引用

普通配置通过以下格式引用凭据：

```text
credential://<name>
```

例如：

```toml
credential = "credential://company_llm"
```

识别规则：配置项下任何以 `credential://` 开头的字符串值都被视为凭据引用，包括嵌套子表中的字段。识别只看值的前缀，与字段名无关。

引用可以通过 `?as=` 覆盖凭据定义中的 `inject_as`，用于同一个凭据需要以不同环境变量名注入的场景：

```toml
credential = "credential://company_llm?as=LLM_API_KEY"
```

上例中的引用指向：

```toml
[credentials.company_llm]
description = "公司 LLM 的访问凭据。"
provider = "env"
name = "COMPANY_LLM_TOKEN"
inject_as = "OPENAI_API_KEY"
```

读取普通字段时，CLI 只返回凭据引用。它不会把引用自动替换成凭据内容。

### 4.4 普通值注入

配置项可以包含一个保留子表 `inject`，声明 `run` 启动目标程序时随凭据一起注入的普通字段：

```toml
[profiles.work.llm]
description = "公司项目默认使用的 LLM。"
endpoint = "https://llm.example.com/v1"
model = "company-model"
credential = "credential://company_llm"

[profiles.work.llm.inject]
OPENAI_BASE_URL = "endpoint"
OPENAI_MODEL = "model"
```

- 键是目标环境变量名，必须是合法的环境变量名。
- 值是本配置项内的字段路径，指向的字段必须存在且为标量（字符串、整数、浮点数或布尔值），注入时转换为字符串。

`inject` 表由 `run` 使用，`list` 和 `show` 正常展示它的内容。

## 5. CLI 设计

命令名称为 `agent-context`。核心查询命令为 `list`、`show`、`get` 和 `find`，凭据管理命令为 `credential`。

### 5.1 路径语法

`get`、`show`、`find` 使用点分路径定位字段：`<配置项>.<字段>`，可继续深入嵌套子表。字段名本身包含点或空格时，用双引号包裹该段：

```bash
agent-context get server."my.field"
```

路径永远不包含 profile 名。访问其他 profile 时使用 `--profile` 参数。

### 5.2 浏览字段

```bash
agent-context list
```

默认列出当前 profile 中的全部配置项、字段类型和说明：

```text
Profile: work — 公司项目的日常开发环境

llm                         公司项目默认使用的 LLM
├─ endpoint                 string
├─ model                    string
├─ credential               credential reference（可用）
└─ inject                   table

ci                          公司项目提交 CI 任务时使用的标签
└─ tags                     array

kubernetes                  日常开发使用的 Kubernetes 测试环境
├─ context                  string
└─ namespace                string
```

查看某个配置项的字段：

```bash
agent-context list llm
```

查看其他 profile：

```bash
agent-context list --profile personal
```

列出所有 profile：

```bash
agent-context list --profiles
```

输出示例：

```text
work       公司项目的日常开发环境（默认）
personal   个人项目使用的环境
```

### 5.3 查看配置项

```bash
agent-context show llm
```

输出：

```text
说明：公司项目默认使用的 LLM。

endpoint:   https://llm.example.com/v1
model:      company-model
credential: company_llm（可用）
inject:     OPENAI_BASE_URL ← endpoint
            OPENAI_MODEL ← model
```

`show` 适合用户和 Agent 阅读。凭据字段只显示引用名称和状态。

### 5.4 获取具体值

```bash
agent-context get llm.endpoint
```

输出：

```text
https://llm.example.com/v1
```

获取数组或复杂数据时，可以要求 JSON 输出：

```bash
agent-context get ci.tags --json
```

输出：

```json
["linux", "self-hosted"]
```

如果路径指向凭据引用，`get` 仍然只返回引用：

```bash
agent-context get llm.credential
```

```text
credential://company_llm
```

### 5.5 搜索配置

```bash
agent-context find llm
```

`find` 搜索配置项名称、字段名称和 `description`：

```text
llm                     公司项目默认使用的 LLM
llm.endpoint            https://llm.example.com/v1
llm.credential          credential://company_llm（可用）
```

搜索默认限定在当前 profile。使用 `--all-profiles` 可以搜索全部环境：

```bash
agent-context find llm --all-profiles
```

### 5.6 凭据状态显示

`list`、`show` 和 `find` 中的凭据状态来自浅检查，不实际取值，因此这些命令不会触发密码管理器交互、系统弹窗或网络请求：

- `env`：检查环境变量是否已设置，显示"可用"或"未设置"。
- `keychain`：不读取凭据项，显示"已配置"。
- `command`：检查 `argv[0]` 是否可在 `PATH` 中找到，显示"已配置"或"命令缺失"。

实际解析验证由 `credential check` 完成（见第 5.8 节）。

### 5.7 检查配置

```bash
agent-context validate
```

检查内容包括：

- TOML 语法是否正确。
- `version` 是否受支持。
- `default_profile` 是否存在。
- Profile 和配置项是否包含 `description`。
- 凭据引用是否指向已定义的凭据，`?as=` 值是否为合法环境变量名。
- Credential provider 是否包含必要字段。
- `inject` 表的键是否为合法环境变量名，值是否指向本配置项中已存在的标量字段。
- 疑似明文 Token、密码或私钥的字段（匹配规则见第 8 节）。
- 在 Unix 类系统上，配置文件权限是否为 `0600`，权限更宽时检查失败。

检查失败时返回非零退出码，并指出可以修改的配置路径。

### 5.8 凭据管理

```bash
agent-context credential list
agent-context credential check <name>
agent-context credential set <name>
```

- `credential list`：列出全部凭据定义、provider 类型和浅检查状态。
- `credential check <name>`：按 provider 实际解析凭据，报告成功或具体失败原因，不输出凭据内容。`command` provider 的解析可能触发密码管理器的交互确认。
- `credential set <name>`：仅用于 `keychain` provider。从终端交互式读取凭据值（不回显），按凭据定义中的 `service` 和 `account` 写入系统凭据存储。对 `env` 和 `command` provider 明确报错，并说明这两类凭据由外部系统管理。

### 5.9 结构化输出

查询命令统一支持 `--json`：

```bash
agent-context list --json
agent-context show llm --json
agent-context find endpoint --json
```

JSON 输出必须保持稳定，供 Agent 和脚本使用。顶层包含配置 `version`；字段至少包含完整路径、数据类型、值和所属 profile。凭据只能包含引用名称、provider 类型和浅检查状态。

## 6. 凭据取用

### 6.1 Provider

第一版支持三种 provider：

#### 环境变量

```toml
[credentials.company_llm]
description = "公司 LLM 的访问凭据。"
provider = "env"
name = "COMPANY_LLM_TOKEN"
inject_as = "OPENAI_API_KEY"
```

适合 CI 或已经通过 Shell 配置的凭据。注意：这类凭据存在于父进程环境中，任何继承该环境的进程（包括 Agent 自身）都能直接读取（见第 10 节威胁模型）。本地日常使用应优先选择 `keychain` 或 `command`。

#### 系统凭据存储

```toml
[credentials.openai_personal]
description = "个人 OpenAI 账户的访问凭据。"
provider = "keychain"
service = "agent-context"
account = "openai-personal"
inject_as = "OPENAI_API_KEY"
```

适合本地日常使用。各平台对接方式：

- macOS：Keychain。
- Windows：Credential Manager。
- Linux：secret-service（如 GNOME Keyring、KWallet）。

在没有可用凭据存储的环境（例如无桌面环境的 Linux 服务器）中，解析该凭据时 CLI 明确报错，并提示改用 `command` 或 `env` provider。

#### 外部命令

```toml
[credentials.production_llm]
description = "生产环境 LLM 的访问凭据。"
provider = "command"
argv = ["op", "read", "op://Engineering/Production LLM/token"]
inject_as = "OPENAI_API_KEY"
```

适合连接 1Password CLI、`pass` 等现有密码管理器。`argv` 必须直接执行，不得经过 Shell 解析。

### 6.2 临时注入

Agent 不应通过 CLI 读取明文凭据。需要凭据时，由 `agent-context` 启动目标程序并临时注入：

```bash
agent-context run --with llm -- llm-client request
```

`--with llm` 表示：

1. 查找当前 profile 中的 `llm` 配置项。
2. 递归找出其中所有 `credential://` 引用。
3. 根据对应 credential 的 provider 取得凭据。
4. 按 `inject_as`（或引用上的 `?as=` 覆盖值）将凭据加入目标进程的环境变量。
5. 按配置项的 `inject` 表加入普通字段的环境变量。
6. 启动 `llm-client request`。

`--with` 可以重复出现，注入多个配置项：

```bash
agent-context run --with llm --with kubernetes -- deploy-tool sync
```

所有注入合并后，如果两个来源（凭据或 `inject` 表）产生相同的环境变量名，CLI 报错并列出冲突双方，不启动目标程序。

进程语义：

- 目标程序的 stdout 和 stderr 直接透传，`agent-context` 不捕获、不缓冲、不改写。
- 目标程序退出后，`run` 透传其退出码；目标程序被信号终止时返回 `128 + 信号编号`。
- `run` 收到 `SIGINT`、`SIGTERM` 时转发给目标进程。
- 启动前的失败（配置错误、凭据不可用、注入冲突）使用第 9 节定义的退出码。

凭据不会写回配置文件，也不会出现在 `agent-context` 自身的输出或日志中。目标程序结束后，临时环境随进程一起消失。

## 7. Agent 使用约定

项目可以在 `AGENTS.md` 中加入以下说明：

```md
User environment information is available through `agent-context`.

- Run `agent-context list --json` to discover available configuration.
- Run `agent-context show <name> --json` before using an unfamiliar entry.
- Use `agent-context get <path>` to retrieve ordinary values.
- Use `agent-context run --with <entry> -- <command>` when credentials are required.
- Never print, log, persist, or summarize resolved credentials.
- Report missing configuration or credentials explicitly.
```

Agent 的推荐流程：

1. 使用 `list --json` 了解当前保存了哪些配置。
2. 根据名称和 `description` 选择相关配置项。
3. 使用 `show` 或 `get` 获取普通字段。
4. 需要凭据时使用 `run --with` 启动目标程序。
5. 配置缺失时明确告知用户，不自动猜测或切换到其他环境。

## 8. 验证规则

核心结构采用严格验证：

- `version` 必须是受支持的整数。
- `default_profile` 必须指向已有 profile。
- Profile 的 `description` 必须是非空字符串。
- 每个配置项的 `description` 必须是非空字符串。
- Credential 的 `description`、`provider` 和 `inject_as` 必须存在。
- Provider 所需字段必须完整。
- `inject` 表的键必须是合法环境变量名，值必须指向本配置项中已存在的标量字段。

自定义数据采用开放验证：

- 除保留键名外，配置项可以包含任意字段。
- 未知字段必须保留并正常显示。
- 新字段必须自动出现在 `list`、`show`、`get` 和 `find` 中。

敏感字段名检查：

- 字段名精确匹配 `token`、`password`、`secret`、`api_key`、`private_key`，或以 `_token`、`_password`、`_secret`、`_api_key`、`_private_key` 结尾时，值必须是 `credential://` 引用，不能是其他字符串。
- 匹配只针对完整字段名和上述后缀，`token_endpoint` 这类名称不受限制。

## 9. 错误处理

CLI 不执行静默回退。常见错误应给出明确原因和下一步操作。

退出码约定：

| 退出码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | 参数或用法错误 |
| 2 | 配置文件错误（语法或验证失败） |
| 3 | Profile 或配置路径不存在 |
| 4 | 凭据不可用或注入冲突 |

`run` 在目标程序启动后透传其退出码（见第 6.2 节）。

配置项不存在：

```text
配置路径“llm.region”不存在。运行“agent-context list llm”查看可用字段。
```

Profile 不存在：

```text
Profile“production”不存在。运行“agent-context list --profiles”查看可用项。
```

凭据不可用：

```text
凭据“company_llm”不可用：环境变量“COMPANY_LLM_TOKEN”未设置。
```

注入冲突：

```text
环境变量“OPENAI_API_KEY”存在注入冲突：credential“company_llm”与 credential“openai_personal”。
```

配置格式错误：

```text
无法读取配置：profiles.work.llm 缺少 description。
```

## 10. 安全要求

威胁模型：agent-context 防止凭据的意外泄露——凭据不落盘、不进入常规查询输出、不出现在错误消息和日志中。它不阻止通过 `run` 启动的程序读取注入的环境变量：目标程序在设计上就是凭据的最终使用者，而目标程序由调用方（包括 Agent）指定。`env` provider 的凭据本来就存在于父进程环境中，任何继承该环境的进程都能读取，因此只适合 CI 等受控环境；本地场景应使用 `keychain` 或 `command` provider，使凭据平时不出现在 Agent 进程的环境中。

具体要求：

- 配置文件不得保存明文 Token、密码或私钥。
- `list`、`show`、`get`、`find` 和 `--json` 不得解析并返回凭据内容。
- `list`、`show`、`find` 的凭据状态只做浅检查，不得触发凭据解析（见第 5.6 节）。
- 错误消息和调试日志不得包含凭据内容。
- `command` provider 不得通过 `sh -c` 或其他 Shell 执行。
- 凭据获取失败时必须终止操作，不得换用其他凭据。
- `run` 只向目标子进程注入所选配置项引用的凭据和 `inject` 表声明的字段。
- 日志可以记录凭据名称、provider、调用时间和目标程序，但不能记录凭据值。

## 11. 第一版范围

第一版包含：

- TOML 配置文件读取。
- Profile 选择。
- 任意自定义配置字段。
- 必填的配置项描述。
- `list`、`show`、`get`、`find` 和 `validate`。
- `credential list`、`credential check` 和 `credential set`。
- 文本与 JSON 输出。
- 退出码约定。
- `env`、`keychain` 和 `command` credential provider。
- `run --with` 临时凭据注入与 `inject` 表普通值注入。

第一版不包含：

- 图形界面。
- 云端配置同步。
- 自动修改用户配置文件（`credential set` 写入系统凭据存储，不改动配置文件）。
- 自动猜测缺失字段。
- 在 CLI 输出中显示明文凭据。

## 12. 完成标准

实现满足以下条件时，可以认为第一版完成：

- 用户新增任意配置项或字段后，CLI 无需修改即可读取并展示。
- `list` 能清楚展示当前保存的配置项、字段类型和描述。
- Agent 可以通过 JSON 输出稳定地发现和读取配置。
- 所有凭据引用都能被验证；`credential check` 能对每个凭据给出成功或具体失败原因。
- 常规查询命令不触发凭据解析。
- 凭据只能通过 `run --with` 临时提供给目标程序；`run` 透传目标程序的退出码和输出。
- 任何查询命令、错误消息和日志都不会泄露凭据内容。
- 配置或凭据不可用时，CLI 按第 9 节的退出码约定返回明确错误。
