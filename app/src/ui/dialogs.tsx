import { createSignal, For, Show } from "solid-js";

interface Question {
  message: string;
  detail?: string;
  choices: string[];
  resolve: (choice: string) => void;
}

const [question, setQuestion] = createSignal<Question>();
const [toasts, setToasts] = createSignal<{ id: number; text: string }[]>([]);
let toastId = 0;

/** Asks the user to pick one of `choices`; the last choice is what Escape picks. */
export function ask(message: string, choices: string[], detail?: string): Promise<string> {
  return new Promise((resolve) => setQuestion({ message, detail, choices, resolve }));
}

export function notify(text: string) {
  const id = ++toastId;
  setToasts((t) => [...t, { id, text }]);
  setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 8000);
}

export function Dialogs() {
  const answer = (choice: string) => {
    const q = question();
    setQuestion(undefined);
    q?.resolve(choice);
  };
  return (
    <>
      <Show when={question()}>
        {(q) => (
          <div
            class="modal-backdrop"
            onKeyDown={(e) => e.key === "Escape" && answer(q().choices[q().choices.length - 1])}
          >
            <div class="modal" role="dialog" aria-modal="true">
              <p class="modal-message">{q().message}</p>
              <Show when={q().detail}>
                <p class="modal-detail">{q().detail}</p>
              </Show>
              <div class="modal-buttons">
                <For each={q().choices}>
                  {(choice, i) => (
                    <button classList={{ primary: i() === 0 }} onClick={() => answer(choice)} autofocus={i() === 0}>
                      {choice}
                    </button>
                  )}
                </For>
              </div>
            </div>
          </div>
        )}
      </Show>
      <div class="toasts">
        <For each={toasts()}>
          {(t) => (
            <div class="toast" onClick={() => setToasts((all) => all.filter((x) => x.id !== t.id))}>
              {t.text}
            </div>
          )}
        </For>
      </div>
    </>
  );
}
