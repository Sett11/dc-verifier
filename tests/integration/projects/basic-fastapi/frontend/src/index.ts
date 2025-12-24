import { ItemSchema } from "./schemas/item";
import { getItems } from "./api/sdk.gen";

export async function loadItems() {
  try {
    const items = await getItems();
    // Use Zod schema to link Zod → API call
    const parsed = ItemSchema.array().parse(items);
    return parsed;
  } catch (error) {
    console.error("Failed to load items:", error);
    throw error; // Re-throw for caller to handle
  }
}



