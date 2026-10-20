export type DictationResult = {
  isFinal: boolean;
  0: { transcript: string };
};

export type DictationResultEvent = Event & {
  results: ArrayLike<DictationResult>;
};

export type DictationErrorEvent = Event & {
  error: string;
  message?: string;
};

export type DictationRecognition = {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  onresult: ((event: DictationResultEvent) => void) | null;
  onerror: ((event: DictationErrorEvent) => void) | null;
  onend: (() => void) | null;
  start: () => void;
  stop: () => void;
  abort: () => void;
};

type DictationConstructor = new () => DictationRecognition;

export function createDictationRecognition(target: Window = window) {
  const speechWindow = target as Window & {
    SpeechRecognition?: DictationConstructor;
    webkitSpeechRecognition?: DictationConstructor;
  };
  const Recognition =
    speechWindow.SpeechRecognition ?? speechWindow.webkitSpeechRecognition;
  return Recognition ? new Recognition() : null;
}

export function dictationTranscript(results: ArrayLike<DictationResult>) {
  let finalText = "";
  let interimText = "";
  for (let index = 0; index < results.length; index += 1) {
    const result = results[index];
    const transcript = result?.[0]?.transcript ?? "";
    if (result?.isFinal) finalText += transcript;
    else interimText += transcript;
  }
  return { finalText, interimText };
}

export function appendDictation(base: string, finalText: string, interimText: string) {
  const prefix = base && !/\s$/.test(base) ? `${base} ` : base;
  return `${prefix}${finalText}${interimText}`;
}
