# Basic FastAPI Example

Этот проект используется как эталонный пример для интеграционных тестов `dc-verifier`.

## Стек

- FastAPI
- Pydantic моделей для схем запросов/ответов
- SQLAlchemy ORM-модель `Item`

## Структура

```text
backend/
  database.py   # SQLAlchemy Base + модель Item
  schemas.py    # Pydantic-схемы ItemBase / ItemCreate / ItemRead
  main.py       # FastAPI-приложение и CRUD-роуты
dc-verifier.toml
```

## Цели для dc-verifier

- Обнаружение Pydantic-схем `ItemCreate` и `ItemRead`.
- Обнаружение ORM-модели `Item` и связи Pydantic ↔ ORM через `from_attributes`.
- Построение цепочек данных для CRUD-роутов `/items/`.



