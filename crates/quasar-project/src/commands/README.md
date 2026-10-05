# Команды изменения

`SceneCommand` задаёт общий контракт создания/удаления объекта, переименования, transform, reparent и назначения компонентов. `SetComponent` заменяет payload того же type ID или добавляет новый; `RemoveComponent` удаляет его. Команда сначала применяется к копии project document, затем проходит полную валидацию; Editor публикует результат только после успеха. UI и MCP используют один контракт. Историю и optimistic revision хранит `EditorDocumentSession`, а не этот crate.
