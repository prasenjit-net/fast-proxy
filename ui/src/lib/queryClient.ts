import { QueryClient } from "@tanstack/react-query";

// Shared cache for every useQuery/useMutation in the app. A 30s staleTime
// keeps the dashboard/config/tasks from refetching on every window focus —
// the WebSocket (LiveContext) is what carries truly live data; queries here
// cover the REST resources fetched on load or after a mutation.
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
    },
  },
});
