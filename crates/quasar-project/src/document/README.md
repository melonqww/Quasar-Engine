# Документы

`ProjectDocument` и `SceneDocument` — постоянные данные проекта, не внутреннее состояние Bevy. В версии 1 хранятся UUID проекта/сцен/объектов, активная сцена, родительские ссылки, локальные transforms и versioned component envelopes. Session revision, ECS Entity и UI-состояние в документы не входят. Схема и границы: [PROJECT_DOCUMENT.md](../../../../Architecture/design/PROJECT_DOCUMENT.md).
