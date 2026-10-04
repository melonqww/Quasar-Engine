# Quasar MCP adapter

This local stdio MCP server connects to one ProjectDocument already loaded by Quasar Editor. The Editor owns the document session and publishes a short-lived loopback endpoint; the adapter does not open project files directly. Writes go through the same validated command and history service used by the Editor UI.

The adapter exposes project/scene queries and commands, plus asset operations: `list_assets`, `start_asset_import`, `start_asset_import_url`, `start_asset_reimport`, `get_asset_job`, `cancel_asset_job`, and `assign_model_asset`. Jobs are shared with the Editor UI and return a job ID for polling/cancellation. Scene mutations still require the current `expected_revision`; model assignment is an undoable `SceneCommand`.

Asset imports accept GLB 2.0, PNG/JPEG and WAV up to 512 MiB. Direct URLs are HTTP(S), have no redirects, and are rejected when they resolve to private/local networks. URL import needs `curl.exe` on Windows or `curl` elsewhere in PATH. Model references are resolved by AssetId and visible in Editor and standalone Player; texture/audio scene components, scripts, play control, and other future Editor features are not included yet.

Run the Editor with `--project-document <path>` to open the document editor, then configure the MCP client to launch `quasar-mcp`.
