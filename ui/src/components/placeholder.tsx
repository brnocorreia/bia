export function Placeholder({ title, children }: { title: string; children?: React.ReactNode }) {
  return (
    <section>
      <h1 className="text-2xl font-semibold">{title}</h1>
      <p className="mt-2 text-sm text-muted-foreground">
        {children ?? "This screen is not built yet."}
      </p>
    </section>
  );
}
