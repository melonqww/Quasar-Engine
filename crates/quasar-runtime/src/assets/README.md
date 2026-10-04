# Загрузка runtime-ресурсов

Загрузка asset references в runtime пока использует Bevy `WorldAssetRoot` с GLB `GltfAssetLabel::Scene(0)`. Project-relative путь получается через validated AssetCatalog, а не хранится в ECS document. Editor и standalone Player задают project root как asset root; source files и AssetId не меняются runtime-ом. Декодированные revision handles, unload/lifetime policy, material/image/audio references и cache пока не сделаны.
