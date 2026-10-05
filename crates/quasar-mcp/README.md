# Quasar MCP adapter

This local stdio MCP server connects to one ProjectDocument already loaded by Quasar Editor. The Editor owns the document session and publishes a short-lived loopback endpoint; the adapter does not open project files directly. Writes go through the same validated command and history service used by the Editor UI.

The adapter exposes project/scene queries and commands, asset operations, revision-checked `read_project_script`/`write_project_script`, and `play_start`/`play_stop`/`play_status`. Generic scene commands can also add, update, or remove typed components. Jobs and edits are shared with the Editor UI; scene mutations require the current document `expected_revision` and are undoable `SceneCommand`s. Script content has its own content revision and Lua validation.

Asset imports accept GLB 2.0, PNG/JPEG, WAV and UTF-8 Lua up to the source limits enforced by the Editor. Direct URLs are HTTP(S), have no redirects, and are rejected when they resolve to private/local networks. URL import needs `curl.exe` on Windows or `curl` elsewhere in PATH. Audio uses the Player's WAV playback and Lua callbacks run only in Player with the current restricted host API. Play tools control the same child Player as Editor UI and lock project changes while active. Manual device/UI/MCP acceptance is still pending.

Run the Editor with `--project-document <path>` to open the document editor, then configure the MCP client to launch `quasar-mcp`.
