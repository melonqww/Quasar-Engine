# Player

Самостоятельное исполнение snapshot. Зависит от `quasar-project` и `quasar-runtime`, напрямую не использует `quasar-editor`.

Stage 0 запускается командой:

```text
quasar-player --snapshot <path-to-quasar.snapshot.json>
```

На этапе 1 документ можно проверить или открыть в минимальном preview:

```text
quasar-player --project-document <path-to-quasar.project.json> --validate-only
quasar-player --project-document <path-to-quasar.project.json>
```

Preview отображает активную сцену и parent-child иерархию; объекты без модели представлены нейтральными кубами. GLB-ссылки загружаются из каталога проекта. Поддерживаются camera, character controller, collider, static/kinematic/dynamic rigid body, door и WAV AudioSource компоненты. Dynamic body учитывает авторскую массу. Kinematic дверь плавно поворачивается вокруг origin объекта (задай origin на петле) до настроенного угла. AudioSource поддерживает loop/one-shot и spatial playback через AudioListener с gain проекта. Щёлкни по окну, чтобы захватить курсор; Escape освобождает его. При E Player делает raycast до 2 м и вызывает `on_interact(object_id)` у назначенного Lua Script Asset. Ограниченный host API — `scene.rotate_y(object_id, radians)`, `door.open()` для взаимодействующего объекта с включёнными Door/Kinematic Body/Collider и `audio.play(asset_id)`; queued commands применяются только после успешного callback.

Editor Play запускает Player отдельным процессом на копии текущего документа и используемых ассетов. Этот процесс не сохраняет runtime-изменения в авторский проект; исходный документ редактируется после Stop. При запуске вручную можно передать `--asset-root <project-root>` для расположения каталога ассетов отдельно от project JSON.

Добавление `--validate-only` проверяет формат документа и наличие всех перечисленных ресурсов без создания окна/GPU. `--run-interaction` выполняет отдельный Stage 0 Lua probe headless; `--benchmark-60s` запускает фиксированный маршрут и печатает median/p95 frame time. `tests/fixtures/stage0-player/` содержит legacy snapshot; `tests/fixtures/stage0-animation/` — отдельную animation-пробу; `tests/fixtures/stage4-gameplay/` — постоянную acceptance-сцену Player/MCP. Player controls: WASD/Space — перемещение и прыжок, E — project Lua interaction; F9/F10 относятся к Stage 0 probe.
