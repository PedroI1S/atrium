// A confirmation before an action that can't be taken back from here (archive a chat, end its
// session). One dialog per window, opened with `askConfirm`; Kobalte's Dialog, as the palette.

import { Dialog } from "@kobalte/core/dialog";
import { Show, createSignal } from "solid-js";

interface Ask {
  title: string;
  body: string;
  confirm: string;
  /** The action can't be undone, or loses something: the button reads as a warning. */
  danger?: boolean;
  resolve: (ok: boolean) => void;
}

const [ask, setAsk] = createSignal<Ask | null>(null);

/** Shows the dialog; resolves true when the user confirms, false on cancel or escape. */
export function askConfirm(opts: Omit<Ask, "resolve">): Promise<boolean> {
  ask()?.resolve(false);
  return new Promise((resolve) => setAsk({ ...opts, resolve }));
}

function answer(ok: boolean): void {
  const current = ask();
  if (!current) return;
  setAsk(null);
  current.resolve(ok);
}

export function ConfirmHost() {
  return (
    <Dialog open={ask() !== null} onOpenChange={(open) => { if (!open) answer(false); }}>
      <Dialog.Portal>
        <Dialog.Overlay class="overlay" />
        <div class="palette-pos">
          <Show when={ask()}>
            {(a) => (
              <Dialog.Content class="palette sheet confirm" data-atrium-part="confirm" role="alertdialog">
                <Dialog.Title class="sheet-title">{a().title}</Dialog.Title>
                <Dialog.Description class="sheet-body">{a().body}</Dialog.Description>
                <div class="sheet-act">
                  <button type="button" class="btn" autofocus onClick={() => answer(false)}>Cancel</button>
                  <button type="button" class="btn pri" classList={{ danger: a().danger }} data-atrium-part="confirm-ok" onClick={() => answer(true)}>{a().confirm}</button>
                </div>
              </Dialog.Content>
            )}
          </Show>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
