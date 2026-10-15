# AI connection

The **AI 연결** dialog prepares three distinct actions. Opening it does not install a skill, read calendar events, copy private data, or change an external client's configuration.

- **AI 초기 설정문 복사** asks the chosen AI to verify tools and current calendar access before treating Dot Calendar as the schedule source. Persistent memory is a separate capability.
- **오늘·향후 7일 일정 브리핑 복사** reads the current Store on click. It includes today and the next six dates in the Windows local timezone, today's completed and incomplete events, and upcoming incomplete events. It expands recurring dates using the existing calendar rules. Titles, dates, times, and completion state enter the system clipboard; notes are excluded. Review the text before pasting it into a web chat. No external transmission is performed.
- **MCP 설정 JSON 복사** generates a `mcpServers.dot-calendar` entry with the current executable's absolute path, `args: ["--mcp"]`, and the resolved shared data directory in `env.DOT_CALENDAR_DATA_DIR`. It does not register or verify a client connection.

The separate **기존 Dot 연결 준비 (로컬 스킬)** button retains the local skill installation flow. The `--connect-dot`, `--disconnect-dot`, `--dot-onboarding`, `--mcp`, and `--shutdown` commands remain supported. A prepared local skill is not proof of an AI account handshake.

## Clients and transports

Official documentation checked on 2026-10-15:

| Client | Documented integration | Manual location / limitation |
|---|---|---|
| Claude Desktop | Local stdio MCP, top-level `mcpServers` with `command`, `args`, and environment variables | `%APPDATA%\Claude\claude_desktop_config.json` ([official local server guide](https://modelcontextprotocol.io/docs/develop/connect-local-servers)) |
| Gemini CLI | stdio subprocess MCP via `mcpServers` | `%USERPROFILE%\.gemini\settings.json`; this is Gemini **CLI**, not the Gemini website ([official MCP configuration](https://geminicli.com/docs/tools/mcp-server/)) |
| ChatGPT web | Custom MCP uses a public HTTPS endpoint or a separately configured supported tunnel | A copied local executable JSON entry alone does not connect a web chat. This app adds no server or tunnel. Use the manual briefing for ordinary web chat ([official connection guide](https://developers.openai.com/plugins/deploy/connect-chatgpt)) |

Only a user-selected installed client should be configured. A folder's presence does not establish installation, consent, version compatibility, or successful tool access. The supported schema above was verified in current documentation; actual Claude/Gemini installation versions and account handshakes were not available for verification in the development environment.

## Why manual configuration

External clients do not share this app's file lock. Comparing bytes and then replacing their configuration leaves a race in which another process's change can be lost. The approved requirements explicitly permit manual JSON guidance when that race cannot be eliminated. Therefore the app does not read, overwrite, back up, merge, or remove external client configuration files automatically.

Before editing, use the chosen client's settings workflow, keep a uniquely named copy of its current file, and preserve all other keys and servers. Add only the `dot-calendar` entry to an existing `mcpServers` object. Do not replace an entire configuration with the copied snippet. Stop if the JSON is malformed, its schema differs, or an existing `dot-calendar` entry contains a different executable or user customization. Resolve that conflict before proceeding. Removal likewise applies only to the deliberately registered entry, after checking the client and executable; never delete the whole configuration.

Restart the selected client as its documentation requires, then verify actual MCP initialization, tool listing, and a read-only calendar query. JSON copying is reported only as copying, never as a successful connection.
