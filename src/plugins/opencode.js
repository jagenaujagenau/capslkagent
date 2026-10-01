
export const CapsLkAgent = async () => {
  // Subagent (task tool) sessions have a parentID; their status changes would
  // otherwise clobber the main session's light.
  const children = new Set();

  return {
    event: async ({ event }) => {
      const props = event?.properties ?? {};
      if (props.info?.id && props.info.parentID) children.add(props.info.id);
      if (props.sessionID && children.has(props.sessionID)) return;

      switch (event?.type) {
        case "session.status": {
          const status = typeof props.status === "string" ? props.status : props.status?.type;
          if (status === "idle") led("off");
          else if (status === "busy" || status === "retry") led("on");
          break;
        }
        case "session.idle":
          led("off");
          break;
        case "permission.asked":
        case "question.asked":
          led("blink");
          break;
        case "permission.replied":
        case "question.replied":
        case "question.rejected":
          led("on");
          break;
      }
    },
  };
};
