import { z } from "zod";

export const ItemSchema = z.object({
  id: z.number().int(),
  title: z.string(),
  description: z.string().optional(),
});

export type Item = z.infer<typeof ItemSchema>;



