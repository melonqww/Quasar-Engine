# Stage 0 animation fixture

## GLB персонажа

- Файл: `assets/Quasar/RobotExpressive.glb` (463,988 bytes).
- Источник: [three.js, pinned revision `b924f0cad4058dc4dde71445c796980c3cd5b5ed`](https://github.com/mrdoob/three.js/tree/b924f0cad4058dc4dde71445c796980c3cd5b5ed/examples/models/gltf/RobotExpressive).
- Прямая ссылка на исходный файл: [RobotExpressive.glb](https://raw.githubusercontent.com/mrdoob/three.js/b924f0cad4058dc4dde71445c796980c3cd5b5ed/examples/models/gltf/RobotExpressive/RobotExpressive.glb).
- Лицензия модели: CC0 1.0 Universal согласно upstream [README модели](https://raw.githubusercontent.com/mrdoob/three.js/b924f0cad4058dc4dde71445c796980c3cd5b5ed/examples/models/gltf/RobotExpressive/README.md).
- Автор модели: Quaternius; модификации: Don McCurdy. CC0 не требует атрибуции, но эти сведения сохраняются для происхождения fixture.
- SHA-256: `047F5E5FB3BB6D378BD1DF16CA6137F2A596C99B3A1B5690B4020C05AAF6F319`.

## Проверенные данные GLB

Файл содержит 74 node, 2 skin и 14 именованных анимационных клипов. Нужные для пробы клипы:

| Роль в пробе | Имя в GLB | Длительность из input accessor | Контрольные моменты |
| --- | --- | ---: | --- |
| Покой | `Idle` | 3.33 с | 0 с, середина (≈1.67 с), конец (≈3.33 с) |
| Ходьба | `Walking` | 0.96 с | 0 с, середина (≈0.48 с), конец (≈0.96 с) |

Для пользовательского сценария `Walk` ссылается на исходный клип `Walking`; имя в самом GLB не переименовывается. Ожидаемые признаки для визуальной проверки: `Idle` — стоячая поза с движением корпуса/головы; `Walking` — цикл ходьбы с чередованием ног, движением стоп и рук. Это пока критерии предстоящей проверки, не подтверждённые кадры из Quasar. Точные длительности и контрольные позы далее считываются из GLB; округлённые значения выше предназначены только для удобства ориентира.

## Границы использования

Это только тестовая модель для локальной технической пробы. Она не является финальным персонажем Quasar, не добавляется в Essentials/Starter и сама по себе не доказывает пригодность импортера или качество анимационного workflow.
