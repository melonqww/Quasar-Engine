# Редакторская сессия

`EditorDocumentSession` владеет открытым `ProjectDocument`, revision, dirty state, save и ограниченной undo/redo history. Мутации и preview принимают expected revision; UI и MCP вызывают этот общий сервис. Selection, layout, камера редактора и открытый transform gesture не сериализуются в игровую сцену. Autosave и разрешение внешних конфликтов пока не реализованы.
