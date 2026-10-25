import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { getApi } from "@/api/client";
import { Placeholder } from "@/components/placeholder";

export const Route = createFileRoute("/_app/workflows")({
  staticData: { nav: { label: "Workflows", order: 10 } },
  component: Workflows,
});

// Placeholder until the Workflows screen lands. It reads the list so the shell exercises the
// API client end to end, including the 401 path.
function Workflows() {
  const workflows = useQuery({
    queryKey: ["workflows"],
    queryFn: async () => {
      const { data } = await getApi().GET("/v1/workflows");
      return data?.workflows ?? [];
    },
  });
  return (
    <Placeholder title="Workflows">
      {workflows.data ? `${workflows.data.length} Workflows.` : "Loading…"}
    </Placeholder>
  );
}
