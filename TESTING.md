## Тестирование dc-verifier

Этот файл описывает основные типы тестов и то, как они запускаются локально и в CI.

### 1. Запуск всех тестов

- **Все юнит- и интеграционные тесты**:

```bash
cargo test --all
```

Эта команда используется и в CI (`.github/workflows/ci.yml`), поэтому любые добавленные тесты автоматически будут запускаться в GitHub Actions.

### 2. Типы тестов

- **Юнит-тесты ядра (`dc-core`)**:
  - Парсинг Python/TypeScript, построение call graph.
  - Обработка импортов и ошибок импортов:
    - `crates/dc-core/tests/python_imports_test.rs` — сценарии `resolve_import_safe`/`resolve_import_cached` и `ImportError`.

- **Юнит-/снэпшот‑тесты репортёров (`dc-cli`)**:
  - `crates/dc-cli/tests/reporters_test.rs` — базовая структура Markdown/JSON‑отчётов и расширенный `summary`.
  - `crates/dc-cli/tests/reporters_snapshot_test.rs` — регрессионные цепочки с намеренными несоответствиями:
    - Frontend‑цепочка **Zod → TypeScript → OpenAPI** с типовым конфликтом.
    - Backend‑цепочка **Pydantic ↔ ORM** с отсутствующим обязательным полем.
  - Эти тесты проверяют, что:
    - JSON‑summary стабилен по количеству цепочек по типам и количеству схем по типам.
    - Markdown‑отчёт содержит ожидаемые разделы и рекомендации для mismatch‑ов.

- **CLI‑тесты strict_imports (`dc-cli`)**:
  - `crates/dc-cli/tests/strict_imports_cli_test.rs`:
    - Проверка, что в нестрогом режиме (`strict_imports = false`) анализ не падает на отсутствующих внешних импортов.
    - Проверка, что в строгом режиме (`strict_imports = true`) CLI корректно обрабатывает отсутствие зависимостей, не падая с паникой.

- **Интеграционные тесты FastAPI/Pydantic/SQLAlchemy (`dc-cli`)**:
  - `crates/dc-cli/tests/integration_fastapi_project_test.rs`:
    - Использует эталонный проект `tests/integration/projects/basic-fastapi`.
    - Генерирует Markdown‑отчёт `.chain_verification_report.md`.
    - Проверяет наличие ожидаемых цепочек (`POST /items/`, `GET /items/`, `GET /items/{item_id}`) и базовой статистики.

- **Интеграционные тесты TypeScript/Zod/OpenAPI (`dc-cli`)**:
  - `crates/dc-cli/tests/integration_ts_zod_openapi_test.rs`:
    - Работает на том же `basic-fastapi`, но с отдельным конфигом `dc-verifier.ts-zod-openapi.toml`.
    - Генерирует JSON‑отчёт `report_ts_zod_openapi.json`.
    - Проверяет наличие расширенного `summary.schemas.by_type` и того, что в отчёте действительно присутствуют схемы разных типов (Pydantic, Zod, TypeScript, OpenAPI).

### 3. Эталонный проект basic-fastapi

Проект находится в `tests/integration/projects/basic-fastapi` и включает:

- **Backend**:
  - Pydantic‑схемы (`backend/schemas.py`).
  - SQLAlchemy‑модели (`backend/database.py` / `backend/models.py`).
  - FastAPI‑роуты (`backend/main.py`).
- **Frontend**:
  - Zod‑схемы (`frontend/src/schemas/item.ts`).
  - Сгенерированный OpenAPI‑клиент (`frontend/src/api/sdk.gen.ts`).
  - Входная точка (`frontend/src/index.ts`).
- **OpenAPI**:
  - `openapi.json` с описанием бекенда.
- **Конфиг**:
  - `dc-verifier.toml` и вспомогательные конфиги, используемые интеграционными тестами.

Этот проект используется как основа для интеграционных и регрессионных тестов, чтобы проверять сквозные цепочки данных:

- **Frontend Zod → TypeScript → OpenAPI → Backend Pydantic → ORM**.

### 4. Добавление новых регрессионных тестов

При добавлении исправлений багов или новых фич:

- **Шаг 1**: воспроизвести сценарий либо:
  - В отдельном временном проекте (по аналогии с `basic-fastapi`), либо
  - В виде целевого юнит-/интеграционного теста в соответствующем крейте.
- **Шаг 2**: добавить проверку в один из существующих тестовых файлов (или создать новый):
  - Для отчётов — предпочтительно добавлять проверки в `crates/dc-cli/tests/reporters_*`.
  - Для импортов/парсеров — в `crates/dc-core/tests/*`.
  - Для end-to-end поведения CLI — в `crates/dc-cli/tests/*`.
- **Шаг 3**: убедиться, что тесты проходят локально (`cargo test --all`) и запускаются в CI.



