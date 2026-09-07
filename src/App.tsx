import { CONTRACT_VERSION, isContractCompatible } from "./api";

export default function App() {
  const compatible = isContractCompatible(CONTRACT_VERSION);
  return (
    <main style={styles.page}>
      <h1 style={styles.title}>SpaceLens</h1>
      <p style={styles.sub}>Complex engine. Simple experience.</p>
      <p style={styles.note}>
        Phase 0 scaffold — contract {CONTRACT_VERSION}{" "}
        {compatible ? "compatible" : "MISMATCH"}. No product UI yet.
      </p>
    </main>
  );
}

const styles: Record<string, React.CSSProperties> = {
  page: {
    fontFamily: "system-ui, -apple-system, 'Segoe UI', sans-serif",
    maxWidth: 640,
    margin: "8vh auto",
    padding: "0 24px",
    color: "#1a1a1a"
  },
  title: { fontSize: 28, fontWeight: 650, margin: "0 0 8px" },
  sub: { fontSize: 15, margin: "0 0 16px", color: "#555" },
  note: { fontSize: 13, color: "#777" }
};
