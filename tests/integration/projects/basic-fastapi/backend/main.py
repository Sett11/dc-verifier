from contextlib import asynccontextmanager
from typing import List
import logging

from fastapi import Depends, FastAPI, HTTPException
from sqlalchemy import create_engine
from sqlalchemy.exc import SQLAlchemyError
from sqlalchemy.orm import Session, sessionmaker

from .database import Base, Item
from .schemas import ItemCreate, ItemRead

logger = logging.getLogger(__name__)


SQLALCHEMY_DATABASE_URL = "sqlite:///./test.db"

engine = create_engine(
    SQLALCHEMY_DATABASE_URL, connect_args={"check_same_thread": False}
)
SessionLocal = sessionmaker(autocommit=False, autoflush=False, bind=engine)


def get_db():
    db = SessionLocal()
    try:
        yield db
    finally:
        db.close()


@asynccontextmanager
async def lifespan(app: FastAPI):
    # Startup: run synchronous DB initialization in thread pool
    import asyncio
    await asyncio.to_thread(Base.metadata.create_all, bind=engine)
    yield
    # Shutdown (if needed)


app = FastAPI(title="Basic FastAPI Example", version="0.1.0", lifespan=lifespan)


@app.post("/items/", response_model=ItemRead)
def create_item(item: ItemCreate, db: Session = Depends(get_db)) -> ItemRead:
    try:
        db_item = Item(title=item.title, description=item.description)
        db.add(db_item)
        db.commit()
        db.refresh(db_item)
        return ItemRead.model_validate(db_item)
    except SQLAlchemyError as e:
        db.rollback()
        logger.exception("Failed to create item")
        raise HTTPException(status_code=500, detail="Failed to create item")


@app.get("/items/", response_model=List[ItemRead])
def list_items(db: Session = Depends(get_db)) -> List[ItemRead]:
    try:
        items = db.query(Item).all()
        return [ItemRead.model_validate(obj) for obj in items]
    except SQLAlchemyError as e:
        logger.exception("Failed to list items")
        raise HTTPException(status_code=500, detail="Failed to retrieve items")


@app.get("/items/{item_id}", response_model=ItemRead)
def get_item(item_id: int, db: Session = Depends(get_db)) -> ItemRead:
    try:
        item = db.query(Item).filter(Item.id == item_id).first()
        if item is None:
            raise HTTPException(status_code=404, detail="Item not found")
        return ItemRead.model_validate(item)
    except SQLAlchemyError as e:
        logger.exception("Failed to get item")
        raise HTTPException(status_code=500, detail="Failed to retrieve item")




