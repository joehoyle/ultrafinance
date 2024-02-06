import { ReactNode } from "react";

export default function PageHeader({ children }: { children: ReactNode }) {
	return <header className="px-4 py-4 font-medium text-lg">{children}</header>
}
