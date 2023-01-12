interface Props {

}

export default function Input({ ...props }: Props & React.HTMLProps<HTMLInputElement>) {
	return <input
		className="text-xs text-gray-700 border rounded-sm w-full border-gray-200 dark:border-transparent dark:rounded dark:bg-white/10 dark:text-gray-300 px-2 py-2 outline-none focus:border-purple-400 focus:dark:border-purple-400"
		{...props}
	/>
}
