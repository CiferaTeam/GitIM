import LightningFS from "@isomorphic-git/lightning-fs";

let fs: LightningFS;

export function getFs(): LightningFS {
  if (!fs) {
    fs = new LightningFS("gitim");
  }
  return fs;
}

export async function readFile(path: string): Promise<string> {
  const f = getFs();
  const data = await f.promises.readFile(path, { encoding: "utf8" });
  return data as string;
}

export async function writeFile(path: string, content: string): Promise<void> {
  const f = getFs();
  await f.promises.writeFile(path, content, "utf8");
}

export async function removeFile(path: string): Promise<void> {
  const f = getFs();
  await f.promises.unlink(path);
}

export async function removeDir(path: string): Promise<void> {
  const f = getFs();
  await f.promises.rmdir(path);
}

export async function readdir(path: string): Promise<string[]> {
  const f = getFs();
  return (await f.promises.readdir(path)) as string[];
}

export async function exists(path: string): Promise<boolean> {
  try {
    const f = getFs();
    await f.promises.stat(path);
    return true;
  } catch {
    return false;
  }
}

export async function mkdir(path: string): Promise<void> {
  const f = getFs();
  try {
    await f.promises.mkdir(path);
  } catch {
    // ignore if exists
  }
}

export async function stat(path: string) {
  const f = getFs();
  return f.promises.stat(path);
}
