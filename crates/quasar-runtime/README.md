# Общий runtime

Исполнение сцены, графика, физика, ввод, звук и gameplay. Player документного проекта строит физику/контроллер из `ProjectDocument`, запускает WAV AudioSource и выполняет ограниченный проектный Lua `on_interact` callback. Lua возвращает проверяемый список host commands только после полного успеха callback; прямого доступа к Bevy World или файлам проекта нет. Runtime используется standalone Player и Stage 0 probe; Editor UI здесь отсутствует.
