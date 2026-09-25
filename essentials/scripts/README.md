# Базовое поведение

Будущие Lua-примеры, например дверь и действие со звуком, со схемами свойств. Нативный контроллер реализуется в runtime/physics.
Lua gameplay scripts for the project essentials pack.

`door.lua` is the Stage 0 interaction fixture. The runtime sandbox exposes only `door.open()` and `audio.play("door")`; it does not expose filesystem, package loading, debug, or arbitrary Bevy world access.
