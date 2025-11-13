/**
 * Converts `FileSystemFileEntry` to `File`
 */
export function fileFromEntry(entry: FileSystemFileEntry): Promise<File> {
  return new Promise<File>((resolve, reject) => entry.file(resolve, reject));
}

/**
 * Reads all entries from `FileSystemDirectoryReader`
 */
export function readAllEntries(reader: FileSystemDirectoryReader): Promise<FileSystemEntry[]> {
  return new Promise<FileSystemEntry[]>((resolve, reject) => {
    const entries: FileSystemEntry[] = [];

    const readBatch = () => {
      reader.readEntries(
        (batch) => {
          if (!batch.length) {
            resolve(entries);
            return;
          }

          entries.push(...(batch as FileSystemEntry[]));
          readBatch();
        },
        (error) => reject(error),
      );
    };

    readBatch();
  });
}

/**
 * Recursively collects all files from `FileSystemDirectoryEntry`
 */
export async function collectFilesFromDirectory(
  directory: FileSystemDirectoryEntry,
): Promise<File[]> {
  const files: File[] = [];
  const stack: FileSystemDirectoryEntry[] = [directory];

  while (stack.length) {
    const current = stack.pop()!;
    const entries = await readAllEntries(current.createReader());

    for (const entry of entries) {
      if (entry.isFile) {
        files.push(await fileFromEntry(entry as FileSystemFileEntry));
      } else if (entry.isDirectory) {
        stack.push(entry as FileSystemDirectoryEntry);
      }
    }
  }

  return files;
}
