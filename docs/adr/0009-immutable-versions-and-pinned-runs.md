# Versions are immutable and Runs are pinned to them

Editing a Workflow creates a new Version. Published Versions never change, Triggers come from the Active Version, and a Run executes the Version it started on until it ends, even if the Active Version changes mid-Run. This gives diff, rollback and audit for free and removes any "migrate in-flight Runs" problem. A Run is retried or resumed on its own Version. Re-running on a newer Version means starting a new Run.
