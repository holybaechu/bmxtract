export enum MessageType {
  INIT,

  // S2C
  WARN,
  ERROR,
  RESULT,
  READ_FILES,
  PROGRESS,

  // C2S
  RENDER,
  READ_FILES_RESPONSE,
}

type MessagePayloadMap = {
  [MessageType.INIT]: undefined;

  // S2C
  [MessageType.WARN]: { message: string };
  [MessageType.ERROR]: { id?: string; error: string };
  [MessageType.RESULT]: { id: string; buffer: ArrayBuffer };
  [MessageType.READ_FILES]: { id: string; paths: string[] };
  [MessageType.PROGRESS]: { id: string; progress: number; stage: string };

  // C2S
  [MessageType.RENDER]: { id: string; bmsText: string; useFloat32: boolean };
  [MessageType.READ_FILES_RESPONSE]: {
    id: string;
    buffers: (ArrayBuffer | undefined)[];
  };
};

export type Message = {
  [T in keyof MessagePayloadMap]: MessagePayloadMap[T] extends undefined
    ? { type: T }
    : { type: T } & MessagePayloadMap[T];
}[keyof MessagePayloadMap];
