
export default function (amp) {
  // The thread state stream is the only signal for "awaiting approval";
  // subscribe once per thread.
  const watched = new Set();
  const forState = (state) =>
    state === "running" ? "on" : state === "awaiting-approval" ? "blink" : "off";

  amp.on("agent.start", (event, ctx) => {
    led("on");
    const id = event?.thread?.id;
    if (id && !watched.has(id) && ctx?.thread?.state?.subscribe) {
      watched.add(id);
      ctx.thread.state.subscribe((state) => led(forState(state)));
    }
  });
  // Must return nothing: returning { action: "continue" } would start a turn.
  amp.on("agent.end", () => {
    led("off");
  });
  amp.onDispose(() => led("off"));
}
