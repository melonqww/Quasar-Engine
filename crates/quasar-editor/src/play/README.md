# Запуск Play

`play.rs` owns the Editor Play service: it packages the open (including dirty) ProjectDocument and referenced assets into a temporary immutable snapshot, starts a sibling standalone Player or `QUASAR_PLAYER_BIN`, polls exit diagnostics, and stops only its child process. The UI and authenticated local MCP session share this service. The document session blocks project writes and script saves while Play is active; the package is removed after Player exits. Process and manual UI/MCP acceptance remain open.
