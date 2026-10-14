import type { IntakeVerdict } from "./generated/IntakeVerdict";
import type { Task } from "./generated/Task";

/** Verdicts of the intake gate (backend intake.rs) that hold a task for the owner. */
const HOLDS: IntakeVerdict[] = ["unclear", "plan_first", "duplicate"];

/** A pending task the intake gate holds (unclear, plan first, a repeat) and the owner has not released. */
export function isHeld(t: Task): boolean {
  return t.status === "pending" && !!t.intake && HOLDS.includes(t.intake.verdict) && !t.intake.released;
}

export const VERDICT_LABEL: Record<IntakeVerdict, string> = { ready: "Ready", unclear: "Unclear", plan_first: "Plan first", duplicate: "Repeats a done task", unsure: "Unsure" };

/** "Unclear", "Plan first", "Repeats #12". */
export function heldReason(t: Task): string {
  const i = t.intake;
  if (!i) return "";
  if (i.verdict === "duplicate") return i.similar !== null ? `Repeats #${i.similar}` : "Repeats a done task";
  return VERDICT_LABEL[i.verdict];
}
