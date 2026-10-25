import { createFileRoute } from "@tanstack/react-router";
import { Placeholder } from "@/components/placeholder";

export const Route = createFileRoute("/_app/runs")({
  staticData: { nav: { label: "Runs", order: 20 } },
  component: () => <Placeholder title="Runs" />,
});
