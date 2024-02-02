export default function ErrorMessage({ children, className }: { children: React.ReactNode, className?: string }) {
	return <div className={`p-2 border rounded border-red-300 bg-red-50/50 text-sm text-red-800 dark:bg-red-900/30 dark:border-red-600 dark:text-white/80 ${className}`}>{children}</div>
}
