import { createFileRoute } from "@tanstack/react-router";
import { Placeholder } from "@/components/placeholder";

export const Route = createFileRoute("/_app/tokens")({
  staticData: { nav: { label: "Tokens", order: 40 } },
  component: () => <Placeholder title="Tokens" />,
});
