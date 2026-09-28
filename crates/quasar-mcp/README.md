# Quasar MCP adapter

This local stdio MCP server connects to one ProjectDocument already loaded by Quasar Editor. The Editor owns the document session and publishes a short-lived loopback endpoint; the adapter does not open project files directly. Writes go through the same validated command and history service used by the Editor UI.

The adapter exposes `get_project_state`, `list_scenes`, and `get_scene`, plus `preview_scene_command`, `apply_scene_command`, `undo`, `redo`, and `save_project`. Mutating operations require the current `expected_revision`; preview validates a candidate without changing the document. The Editor session revision starts at `0` and advances after accepted edits, undo, redo, and changed transform gestures.

The first write slice covers object creation, subtree deletion, rename, transform, and reparent commands. Asset import, scripts, play control, and other future Editor features are not included yet.

Run the Editor with `--project-document <path>` to open the document editor, then configure the MCP client to launch `quasar-mcp`.
