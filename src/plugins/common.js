import { spawn } from "node:child_process";

const EXE = __EXE__;
const ARGS = __ARGS__;

let queue = Promise.resolve();
let last = null;

// Run `capslkagent <action>`, one at a time and in order, never throwing into
// the agent. Repeated on/off are skipped; blink always goes through because a
// keypress may have ended the previous one.
function led(action) {
  if (action !== "blink" && action === last) return queue;
  last = action;
  queue = queue.then(
    () =>
      new Promise((resolve) => {
        try {
          const child = spawn(EXE, [...ARGS, action], { stdio: "ignore", windowsHide: true });
          child.on("error", resolve);
          child.on("exit", resolve);
        } catch {
          resolve();
        }
      }),
  );
  return queue;
}
