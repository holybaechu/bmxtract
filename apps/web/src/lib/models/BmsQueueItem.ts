export class BmsQueueItem {
  name: string;
  files: File[];
  chart?: string;
  resultBuffer?: ArrayBuffer;
  progress?: number;
  stage?: string;
  missingFiles?: string[];

  constructor(name: string, files: File[], chart?: string) {
    this.name = name;
    this.files = files;
    this.chart = chart;
    this.progress = 0;
    this.stage = "Queued";
  }
}
