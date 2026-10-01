
// Shared by pi and omp (oh-my-pi), which expose the same extension API.
export default function (pi) {
  // pi emits agent_settled once retries/queued work are done; omp only has
  // agent_end. Once settled is seen, agent_end is ignored.
  let settled = false;
  // omp runs extensions for subagents too; only the main agent drives the light.
  const main = (ctx) => !ctx?.agent?.kind || ctx.agent.kind === "main";

  pi.on("agent_start", (_event, ctx) => {
    if (main(ctx)) led("on");
  });
  pi.on("agent_settled", (_event, ctx) => {
    settled = true;
    if (main(ctx)) led("off");
  });
  pi.on("agent_end", (_event, ctx) => {
    if (!settled && main(ctx)) led("off");
  });
  // omp: a tool call is waiting for approval.
  pi.on("tool_approval_requested", (_event, ctx) => {
    if (main(ctx)) led("blink");
  });
  pi.on("tool_approval_resolved", (_event, ctx) => {
    if (main(ctx)) led("on");
  });
  // pi: an extension dialog (confirm/select/input) is waiting for the user.
  pi.on("ui_prompt_start", () => {
    led("blink");
  });
  pi.on("ui_prompt_end", () => {
    led("on");
  });
  pi.on("session_shutdown", () => led("off"));
}
