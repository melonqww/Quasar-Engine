# Сохранение и восстановление

`ProjectDocument` читается после JSON decode и schema validation. Сохранение пишет один project bundle в sibling temporary file, вызывает `sync_all`, затем заменяет целевой файл; ошибка до замены сохраняет предыдущую версию. Stage 1 пока не реализует backup/journal, autosave или восстановление набора раздельных файлов. Stage 0 snapshot использует тот же atomic writer, но сохраняет свой отдельный временный формат.
