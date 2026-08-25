import { useState } from "react";

export interface SelectionState {
  selectedId: string | null;
  hoverId: string | null;
  setSelected: (id: string | null) => void;
  setHover: (id: string | null) => void;
  toggleSelected: (id: string) => void;
}

export function useSelection(): SelectionState {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hoverId, setHoverId] = useState<string | null>(null);

  return {
    selectedId,
    hoverId,
    setSelected: setSelectedId,
    setHover: setHoverId,
    toggleSelected: (id: string) =>
      setSelectedId((cur) => (cur === id ? null : id)),
  };
}
