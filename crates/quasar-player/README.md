# Player

Самостоятельное исполнение snapshot. Зависит от `quasar-project` и `quasar-runtime`, напрямую не использует `quasar-editor`.

Stage 0 запускается командой:

```text
quasar-player --snapshot <path-to-quasar.snapshot.json>
```

Добавление `--validate-only` проверяет формат snapshot и наличие всех перечисленных ресурсов без создания окна/GPU. `--run-interaction` выполняет Lua `on_interact()` headless и печатает подготовленные host-команды для быстрой Lua-итерации. `--benchmark-60s` выполняет 5-секундный warmup и фиксированный 60-секундный маршрут W-D-S-A, печатает median/p95 frame time и автоматически закрывает окно; пиковую память процесса во время запуска нужно замерять отдельно. Snapshot задаёт `assets_directory` и относительные пути к модели, аудио и Lua; каждый файл должен оставаться внутри каталога ресурсов проекта. Если присутствует Stage 0 animation extension, Player также разрешает все объявленные animation asset IDs внутри asset root и останавливается с явной ошибкой при отсутствующем/внешнем файле. `tests/fixtures/stage0-player/` содержит запускаемый обычный пример; `tests/fixtures/stage0-animation/` — изолированный fixture с ключами двери и ссылками на скелетные клипы для технической пробы. Player controls: WASD/Space — перемещение и прыжок, E — Lua-взаимодействие, F10 — Stop аудио, F9 — restart фона.
