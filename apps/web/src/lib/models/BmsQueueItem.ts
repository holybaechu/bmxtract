export class BmsQueueItem {
  name: string;
  files: File[];
  fileIndex: Map<string, File>;
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

    this.fileIndex = new Map();
    for (const file of files) {
      const lowerName = file.name.toLowerCase();
      this.fileIndex.set(lowerName, file);

      const nameWithoutExt = lowerName.substring(0, lowerName.lastIndexOf("."));
      if (!this.fileIndex.has(nameWithoutExt)) {
        this.fileIndex.set(nameWithoutExt, file);
      }
    }
  }
}
