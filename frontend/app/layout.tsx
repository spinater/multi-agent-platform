import type { Metadata } from "next";
import { Geist, Geist_Mono, IBM_Plex_Sans_Thai, JetBrains_Mono } from "next/font/google";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

const jetbrainsMono = JetBrains_Mono({
  variable: "--font-jetbrains-mono",
  subsets: ["latin"],
});

/** Thai glyphs: Geist and the mono fonts have none, so the stacks fall through to IBM Plex Sans Thai, whose
 * geometric shapes and weights sit well beside Geist (instead of the system's Thai font). */
const plexThai = IBM_Plex_Sans_Thai({
  variable: "--font-thai",
  subsets: ["thai", "latin"],
  weight: ["400", "500", "600", "700"],
});

export const metadata: Metadata = {
  title: "MAP · Multi-Agent Platform",
  description: "Run, watch and orchestrate coding agents",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" className={`${geistSans.variable} ${geistMono.variable} ${jetbrainsMono.variable} ${plexThai.variable} h-full antialiased`}>
      <body className="flex min-h-full flex-col font-sans text-[13px]">{children}</body>
    </html>
  );
}
