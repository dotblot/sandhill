import { createSignal, For, Show } from "solid-js";

interface Question {
  message: string;
  detail?: string;
  choices: string[];
  resolve: (choice: string) => void;
}

interface TextQuestion {
  message: string;
  detail?: string;
  value: string;
  ok: string;
  resolve: (text: string | undefined) => void;
}

const [question, setQuestion] = createSignal<Question>();
const [textQuestion, setTextQuestion] = createSignal<TextQuestion>();
const [toasts, setToasts] = createSignal<{ id: number; text: string; kind: "error" | "info" }[]>([]);
let toastId = 0;

/** Asks the user to pick one of `choices`; the last choice is what Escape picks. */
export function ask(message: string, choices: string[], detail?: string): Promise<string> {
  return new Promise((resolve) => setQuestion({ message, detail, choices, resolve }));
}

/** Asks the user for a line of text, starting from `value`; undefined if they cancel. */
export function askText(message: string, value: string, ok: string, detail?: string): Promise<string | undefined> {
  return new Promise((resolve) => setTextQuestion({ message, detail, value, ok, resolve }));
}

/** Shows a message for a while: something that went wrong, or with `info`, something done. */
export function notify(text: string, kind: "error" | "info" = "error") {
  const id = ++toastId;
  setToasts((t) => [...t, { id, text, kind }]);
  setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 8000);
}

export function Dialogs() {
  const answer = (choice: string) => {
    const q = question();
    setQuestion(undefined);
    q?.resolve(choice);
  };
  const answerText = (text: string | undefined) => {
    const q = textQuestion();
    setTextQuestion(undefined);
    q?.resolve(text);
  };
  let field: HTMLInputElement | undefined;
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
      <Show when={textQuestion()}>
        {(q) => (
          <div class="modal-backdrop" onKeyDown={(e) => e.key === "Escape" && answerText(undefined)}>
            <form
              class="modal"
              role="dialog"
              aria-modal="true"
              onSubmit={(e) => {
                e.preventDefault();
                answerText(field?.value ?? "");
              }}
            >
              <p class="modal-message">{q().message}</p>
              <Show when={q().detail}>
                <p class="modal-detail">{q().detail}</p>
              </Show>
              <input
                class="modal-input"
                value={q().value}
                ref={(el) => {
                  field = el;
                  // Ready to type over; a file's name, before its extension.
                  queueMicrotask(() => {
                    el.focus();
                    const extension = /\.[a-z0-9]{1,5}$/i.exec(el.value);
                    el.setSelectionRange(0, extension ? extension.index : el.value.length);
                  });
                }}
              />
              <div class="modal-buttons">
                <button type="submit" class="primary">
                  {q().ok}
                </button>
                <button type="button" onClick={() => answerText(undefined)}>
                  Cancel
                </button>
              </div>
            </form>
          </div>
        )}
      </Show>
      <div class="toasts">
        <For each={toasts()}>
          {(t) => (
            <div class="toast" classList={{ info: t.kind === "info" }} onClick={() => setToasts((all) => all.filter((x) => x.id !== t.id))}>
              {t.text}
            </div>
          )}
        </For>
      </div>
    </>
  );
}
