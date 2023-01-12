import { ReactNode } from "react";

export default function PageHeader( { children } : { children: ReactNode }) {
	return <header className="px-10 py-4 font-medium text-lg border-b border-b-gray-200">{children}</header>
}
