import type { ReactNode } from "react";

export const metadata = {
  title: "Tree Simulator",
  description: "Hex-grid forest ecology simulation",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body style={{ margin: 0, background: "#0b0e13", color: "#e6edf3" }}>{children}</body>
    </html>
  );
}
