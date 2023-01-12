export default function LoadingList({ number = 3 }: { number?: number }) {
	return <>
		{Array(number).fill(undefined).map((item, i) => (
			<LoadingListItem key={i} />
		))}
	</>
}

function LoadingListItem() {
	return <li className="flex animate-pulse">
		<div className="flex flex-1 h-8 items-stretch space-x-3 my-2 bg-white/50 border-b border-b-slate-50 font-medium text-xs text-slate-600 hover:text-purple-900">
			<div className="w-8 flex-0 bg-gray-100 rounded"></div>
			<div className="flex-1 text-left flex flex-col bg-gray-100 rounded">
				&nbsp;
			</div>
		</div>
	</li>
}
