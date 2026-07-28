export interface TreeNode {
  name: string;
  path: string;
  kind: "file" | "directory";
  children: TreeNode[];
  depth: number;
}

export function buildFileTree(paths: string[]): TreeNode {
  const root: TreeNode = {
    name: "",
    path: "",
    kind: "directory",
    children: [],
    depth: -1,
  };

  for (const path of paths) {
    const segments = path.split("/");
    let current = root;
    for (let i = 0; i < segments.length; i += 1) {
      const segment = segments[i];
      const isFile = i === segments.length - 1;
      const childPath = segments.slice(0, i + 1).join("/");
      let child = current.children.find((c) => c.name === segment);
      if (!child) {
        child = {
          name: segment,
          path: childPath,
          kind: isFile ? "file" : "directory",
          children: [],
          depth: i,
        };
        current.children.push(child);
      }
      current = child;
    }
  }

  return root;
}
