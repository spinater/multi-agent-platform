/** How to attach to an agent's tmux session by hand: with `workspace.tmux_socket` set, MAP's sessions live on that
 * private server (`tmux -L <name>`), which a plain `tmux attach` does not reach. */
export function attachCommand(socket: string | null | undefined, session: string): string {
  return `tmux ${socket ? `-L ${socket} ` : ""}attach -t '=${session}'`;
}
