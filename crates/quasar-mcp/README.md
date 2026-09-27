# Quasar MCP adapter

This local stdio MCP server is a read-only adapter to one ProjectDocument already loaded by Quasar Editor. The Editor owns the document session and publishes a short-lived loopback endpoint; the adapter does not open or modify project files.

The first slice exposes `get_project_state`, `list_scenes`, and `get_scene`. Session revision starts at `0` because document-edit commands are introduced in a later stage.

Run the Editor with `--project-document <path>` to open this technical Stage 1 session, then configure the MCP client to launch `quasar-mcp`. The Editor window still hosts the Stage 0 compatibility viewport; project-document visualization in the Editor UI is not part of this slice.
