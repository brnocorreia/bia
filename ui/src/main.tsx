import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { configureApi } from "@/api/client";
import { session } from "@/auth/session";
import { loadConfig, setConfig } from "@/config";
import { createAppRouter } from "./router";
import "./index.css";

async function main() {
  const root = createRoot(document.getElementById("root")!);
  try {
    const config = await loadConfig();
    setConfig(config);
    configureApi(config.apiUrl);
  } catch (error) {
    root.render(
      <p role="alert" className="p-6 text-destructive">
        {error instanceof Error ? error.message : "Cannot start the UI."}
      </p>,
    );
    return;
  }

  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: 5_000 } },
  });
  // Cached responses belong to the Token that fetched them.
  session.subscribe(() => queryClient.clear());
  const router = createAppRouter(queryClient);

  root.render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </StrictMode>,
  );
}

void main();
