# Basic FastAPI Example

This project is used as a reference example for `dc-verifier` integration tests.

## Stack

- FastAPI
- Pydantic models for request/response schemas
- SQLAlchemy ORM model `Item`

## Structure

```text
backend/
  database.py   # SQLAlchemy Base + Item model
  schemas.py    # Pydantic schemas ItemBase / ItemCreate / ItemRead
  main.py       # FastAPI application and CRUD routes
dc-verifier.toml
```

## Goals for dc-verifier

- Detect Pydantic schemas `ItemCreate` and `ItemRead`.
- Detect ORM model `Item` and Pydantic ↔ ORM connection via `from_attributes`.
- Build data chains for CRUD routes `/items/`.


