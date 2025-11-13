export class BmsQueueItem {
  name: string;
  chart?: string;
  fileIndex: Map<string, File>;
  stage?: string;
  progress?: number;
  resultBuffer?: ArrayBuffer;

  constructor(name: string, files: File[], chart?: string) {
    this.name = name;
    this.chart = chart;
    this.stage = "Queued";
    this.progress = 0;

    this.fileIndex = new Map();
    for (const file of files) {
      const lowerName = file.name.toLowerCase();
      if (lowerName.endsWith(".wav") || lowerName.endsWith(".ogg") || lowerName.endsWith(".mp3")) {
        this.fileIndex.set(file.name.split(".").slice(0, -1).join("."), file);
      } else {
        this.fileIndex.set(file.name, file);
      }
    }
  }
}
