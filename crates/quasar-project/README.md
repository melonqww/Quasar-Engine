# Данные проекта

`quasar-project` содержит авторские документы, stable IDs, валидацию и файловое сохранение. Он не зависит от UI, GPU или runtime Entity.

Stage 1 вводит отдельную версионированную модель `ProjectDocument` → `SceneDocument` → scene objects. UUID сохраняются между открытиями; иерархия задаётся `parent_id`, transforms хранятся локально, а component payload имеет собственный type ID и schema version. Один JSON project bundle сохраняется атомарно после validation. Session revision не сериализуется.

`ProjectSnapshot` остаётся совместимым форматом Stage 0 fixtures и probe Player. Его `animation` extension, fixed object kinds и ссылки по относительным путям не становятся частью постоянной SceneDocument схемы. Отличия и намеренный предел первой версии описаны в [проектном формате](../../Architecture/design/PROJECT_DOCUMENT.md).
