import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { unwrap } from "./request";

export type QueueInfo = components["schemas"]["QueueInfo"];
export type MessageInfo = components["schemas"]["MessageInfo"];

/** Which Messages of a Queue to list: every one, only dead letters, or only the others. */
export type MessageFilter = "all" | "dead" | "live";

export const queueKeys = {
  all: ["queues"] as const,
  list: () => ["queues", "list"] as const,
  one: (name: string) => ["queues", "one", name] as const,
  messages: (name: string, filter: MessageFilter) => ["queues", "messages", name, filter] as const,
};

/** The most dead letters one replay or purge takes, and so the page size. */
export const PAGE_SIZE = 200;

export function useQueues() {
  return useQuery({
    queryKey: queueKeys.list(),
    queryFn: async () => unwrap(await getApi().GET("/v1/queues")).queues,
  });
}

export function useQueue(name: string) {
  return useQuery({
    queryKey: queueKeys.one(name),
    queryFn: async () =>
      unwrap(await getApi().GET("/v1/queues/{name}", { params: { path: { name } } })),
  });
}

export function useMessages(name: string, filter: MessageFilter) {
  return useInfiniteQuery({
    queryKey: queueKeys.messages(name, filter),
    initialPageParam: undefined as number | undefined,
    queryFn: async ({ pageParam }) =>
      unwrap(
        await getApi().GET("/v1/queues/{name}/messages", {
          params: {
            path: { name },
            query: {
              dead_letters: filter === "all" ? undefined : filter === "dead",
              after: pageParam,
              limit: PAGE_SIZE,
            },
          },
        }),
      ),
    getNextPageParam: (page) => page.next_after ?? undefined,
  });
}

/**
 * Replays or purges dead letters of a Queue. Either one changes the counts and Messages of the
 * Queue and, for a replay, of the Queues the dead letters came from, so every Queue is refetched.
 */
export function useDeadLetterAction(name: string, action: "replay" | "purge") {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (ids: number[]) => {
      const params = { params: { path: { name } }, body: { ids } };
      if (action === "replay") {
        const replayed = unwrap(
          await getApi().POST("/v1/queues/{name}/dead-letters/replay", params),
        );
        return replayed.replayed.length;
      }
      return unwrap(await getApi().POST("/v1/queues/{name}/dead-letters/purge", params)).purged;
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queueKeys.all }),
  });
}
