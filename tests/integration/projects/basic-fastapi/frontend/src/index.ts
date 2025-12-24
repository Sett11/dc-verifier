import { ItemSchema } from "./schemas/item";
import { getItems } from "./api/sdk.gen";

export async function loadItems() {
  const items = await getItems();
  // Использование Zod-схемы, чтобы трекер связал Zod → API вызов
  const parsed = ItemSchema.parse(items);
  return parsed;
}



