import { createFileRoute } from "@tanstack/react-router";
import { Placeholder } from "@/components/placeholder";

export const Route = createFileRoute("/_app/secrets")({
  staticData: { nav: { label: "Secrets", order: 30 } },
  component: () => <Placeholder title="Secrets" />,
});
