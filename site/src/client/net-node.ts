/** A step of a `<decision-net>`, read from its server-rendered button. */
export interface Node {
  el: HTMLElement;
  id: string;
  depth: number;
  from: string[];
  branch: boolean;
  fact: boolean;
  bars: HTMLElement[];
  nums: HTMLElement[];
  ps: number[];
  x: number;
  y: number;
  state: string;
  k: number;
}

export function readNode(el: HTMLElement): Node {
  const nums = [...el.querySelectorAll<HTMLElement>(".on")];
  return {
    el,
    id: el.dataset["id"] ?? "",
    depth: Number(el.dataset["depth"]),
    from: (el.dataset["from"] ?? "").split(" ").filter(Boolean),
    branch: el.dataset["branch"] !== undefined,
    fact: el.classList.contains("fact"),
    bars: [...el.querySelectorAll<HTMLElement>(".ob span")],
    nums,
    ps: nums.map((num) => Number(num.dataset["p"])),
    x: 0,
    y: 0,
    state: "",
    k: -1,
  };
}
