# Каталог ассетов

`assets::AssetId` — стабильный UUID, не зависящий от имени и содержимого. `AssetMetadata` хранится в соседнем `<source>.meta.json`; каталог восстанавливается сканированием `Assets/**/*.meta.json`, сообщает о повреждённых sidecar, missing source и конфликтующих ID. Исходные пути проекта относительны и не могут выходить за корень; ссылки и subresource IDs появятся при подключении ресурсов к SceneDocument.

Первая metadata schema version хранит тип, путь исходника, importer ID/version, настройки, URL-источник, автора, лицензию и список производных файлов. Отсутствующая лицензия означает «неизвестна». Декодирование и публикация импорта находятся в `quasar-editor/src/import`; runtime handles и загрузка в runtime — в `quasar-runtime/src/assets`.
