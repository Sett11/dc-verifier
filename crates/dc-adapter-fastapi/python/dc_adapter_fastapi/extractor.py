"""
Извлекает метаданные из FastAPI приложения без запуска сервера.
Использует introspection API FastAPI.
"""
import importlib.util
import inspect
import sys
import logging
from pathlib import Path
from typing import Optional
from fastapi import FastAPI
from fastapi.routing import APIRoute

logger = logging.getLogger(__name__)


class FastApiExtractor:
    """Извлекает информацию о FastAPI приложении"""
    
    def __init__(self, app_path: str):
        self.app_path = Path(app_path)
        self.app: Optional[FastAPI] = None
    
    def load_app(self) -> FastAPI:
        """Загружает FastAPI приложение из файла"""
        if self.app is not None:
            return self.app
        
        # Динамический импорт модуля
        spec = importlib.util.spec_from_file_location("app", self.app_path)
        if spec is None or spec.loader is None:
            raise ValueError(f"Cannot load module from {self.app_path}")
        
        module = importlib.util.module_from_spec(spec)
        # Добавляем модуль в sys.modules для корректной работы относительных импортов
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        
        # Ищем app в модуле
        if hasattr(module, "app") and isinstance(module.app, FastAPI):
            self.app = module.app
        else:
            raise ValueError(f"No FastAPI app found in {self.app_path}")
        
        return self.app
    
    def extract_routes(self) -> list[dict]:
        """Извлекает все routes из FastAPI приложения"""
        if self.app is None:
            self.load_app()
        
        routes = []
        for route in self.app.routes:
            if isinstance(route, APIRoute):
                # Обрабатываем возможные исключения при получении информации о файле
                try:
                    handler_file = inspect.getfile(route.endpoint)
                    handler_line = inspect.getsourcelines(route.endpoint)[1]
                except (TypeError, OSError) as e:
                    handler_file = None
                    handler_line = None
                    logger.warning(
                        f"Не удалось получить информацию о файле для {route.endpoint}: {e}"
                    )
                
                # Сохраняем все HTTP методы без мутации route.methods
                methods = list(route.methods) if route.methods else ["GET"]
                
                routes.append({
                    "path": route.path,
                    "methods": methods,
                    "handler": route.endpoint.__name__,
                    "handler_file": handler_file,
                    "handler_line": handler_line,
                    "dependant": route.dependant,
                })
        
        return routes

