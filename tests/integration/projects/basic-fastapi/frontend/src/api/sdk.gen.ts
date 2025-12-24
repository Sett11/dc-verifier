// Simplified generated SDK client for OpenAPI

export interface Item {
  id: number;
  title: string;
  description?: string | null;
}

interface Client {
  get<T>(args: { url: string }): Promise<T>;
}

// Pretend this client is provided by the SDK runtime
declare const client: Client;

export function getItems(): Promise<Item[]> {
  // Pattern recognizable by TypeScriptCallGraphBuilder::extract_api_info_from_sdk_function
  return (client as Client).get<Item[]>({
    url: "/items/",
  });
}



