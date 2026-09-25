# Модули движка

Начальный Cargo workspace содержит четыре Rust crates: quasar-project, quasar-runtime, quasar-editor и quasar-player. В runtime/editor/player временно добавлены минимальные точки совместимости для проверки выбранного стека; это ещё не реализация подсистем. Зависимости: editor → runtime → project; player → runtime и project. Project не зависит от Bevy, runtime не зависит от editor.
