# Команды изменения

`SceneCommand` задаёт общий контракт создания/удаления объекта, переименования, transform и reparent. Команда сначала применяется к копии project document, затем проходит полную валидацию; Editor публикует результат только после успеха. UI и MCP используют один контракт. Историю и optimistic revision хранит `EditorDocumentSession`, а не этот crate.
